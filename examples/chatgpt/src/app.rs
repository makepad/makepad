use crate::{makepad_error_log::*};
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
                text: "_________"
            }
            message_label = <Label> {
                draw_label: {
                    color: #f
                },
                label: "beep beep boop."
            }
            send_button = <Button> {
                draw_icon:{
                    //svg_file: dep("crate://self/resources/Icon_Redo.svg")
                    // svg_path:"M17.218,2.268L2.477,8.388C2.13,8.535,2.164,9.05,2.542,9.134L9.33,10.67l1.535,6.787c0.083,0.377,0.602,0.415,0.745,0.065l6.123-14.74C17.866,2.46,17.539,2.134,17.218,2.268 M3.92,8.641l11.772-4.89L9.535,9.909L3.92,8.641z M11.358,16.078l-1.268-5.613l6.157-6.157L11.358,16.078z"
                   //path:"M0,0 L20.0,0.0 L20.0,20.0 Z"
                }
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
    
    #[rust] latest_reply: String,
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
    }
}

impl App{
    // EVENT-BASED: sends message, has no relationship with response. Response will be received as an event
    // and processed by handle_event.
    fn send_message(cx_ref:CxRef, ui:WidgetRef) {
        let message = ui.get_text_input(id!(message_input)).get_text(); // TODO

        let mut cx = cx_ref.0.borrow_mut(); // need to cleanup this

        // WIP
        let completion_url = format!("{}/chat/completions", OPENAI_BASE_URL);
        let mut request = HttpRequest::new(completion_url);
        request.set_header("Content-Type", "application/json");
        request.set_body(ChatPrompt {
            message,
        });

        cx.http_request(request);
    }

    // ASYNC: sends message, awaits for response and updates the text value.
    async fn send_message_async(_cx_ref: CxRef, _ui: WidgetRef){
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
            let chat_response = event.response.get_body::<ChatResponse>().unwrap();

            let label = self.ui.get_label(id!(message_label));
            label.set_label(&chat_response.message); // iterate over choices and choose a message; 
            label.redraw(cx);
        }
        
        let actions = self.ui.handle_widget_event(cx, event);
        
        if self.ui.get_button(id!(send_button)).clicked(&actions) {
            cx.spawner().spawn(async { // this doesn't have to be async you could just call send_message
                Self::send_message(cx.get_ref(), self.ui.clone())
            }).unwrap();
        }
    }
}

#[derive(DeBin, SerBin, PartialEq, Debug)]
struct ChatPrompt {
    pub message: String,
    // some other stuff like max_tokens, temprature, etc.
}


#[derive(DeBin, SerBin, PartialEq, Debug)]
struct ChatResponse {
    pub message: String,
    // this actually looks like:
    //   "choices": [{
    //     "index": 0,
    //     "message": {
    //       "role": "assistant",
    //       "content": "\n\nHello there, how may I assist you today?",
    //     },
    //     "finish_reason": "stop"
    //   }],
}