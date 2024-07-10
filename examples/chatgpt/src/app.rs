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
	    /*
            align: {
                x: 0.5,
                y: 1.0
            },
	    */
	    padding: {
		left: 100.0,
		top: 100.0,
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
                text: r#"Lorem ipsum dolor sit amet, consectetur adipiscing elit. Ut vel velit ac urna imperdiet fermentum. Nullam eu quam elit. Cras condimentum purus quam, ac pellentesque arcu facilisis placerat. Maecenas accumsan sem quis mattis dignissim. Integer eget lacinia eros. Donec hendrerit nisl et ligula ornare, quis commodo lacus hendrerit. Morbi facilisis risus sit amet vestibulum malesuada. Duis nec ligula quis enim accumsan accumsan a et felis. Fusce orci nisl, scelerisque ac elit ut, eleifend sodales nisi."#
                /*

Etiam scelerisque, turpis eget finibus convallis, diam sapien gravida erat, eu ornare dolor mauris quis leo. Morbi eget porttitor purus, a sagittis erat. Duis porttitor bibendum porttitor. Quisque aliquam eros quam, at interdum ipsum elementum non. Morbi mollis nunc ut luctus iaculis. Mauris turpis mauris, ultrices eget pharetra at, finibus pellentesque magna. Aliquam pulvinar cursus erat, non interdum lorem accumsan sit amet. Ut placerat ante eu mauris consequat, non volutpat leo hendrerit. Nam volutpat malesuada nunc. Quisque tincidunt malesuada est, vitae faucibus massa egestas vitae. Integer at purus elit. Proin nec ipsum arcu. Integer sit amet arcu a libero posuere congue. Cras eu venenatis lacus, nec fermentum eros. Vivamus ut tristique mauris, a porta ipsum.

Integer eu enim finibus, aliquet nunc sit amet, tincidunt quam. Proin accumsan massa in lacus hendrerit, ut vulputate nisl blandit. Quisque tincidunt hendrerit libero at congue. Sed ultrices, nunc in auctor porta, dolor sem commodo arcu, ut mollis tortor arcu eu mi. Pellentesque in enim non risus fringilla aliquam. Cras quis erat non risus maximus volutpat. Interdum et malesuada fames ac ante ipsum primis in faucibus. Nullam iaculis interdum felis, eget vestibulum libero feugiat in. Suspendisse nibh metus, tempor eu viverra sed, semper eget risus. Praesent mauris quam, tempor id lectus vitae, consequat bibendum ligula. Nunc eu nulla accumsan, pharetra tellus id, egestas tellus. In pretium augue eu quam tempus, at congue quam rutrum. Etiam quis mauris sed enim tristique rhoncus quis a massa. In et neque lacus.
"#,
*/
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
