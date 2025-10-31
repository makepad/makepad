use crate::makepad_live_id::*;
use makepad_widgets::*;

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
        
    App = {{App}} {
        ui: <Window> {body = {
            show_bg: true
            flow: Down,
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
                
        let code = script!{
            use mod.net
            use mod.fs
            
            let send_local = |message|{
                let req = net.HttpRequest{
                    url: "http://127.0.0.1:8080/v1/chat/completions"
                    method: net.HttpMethod.POST
                    headers:{
                        "Content-Type": "application/json"
                        "Authorization": "Bearer "+fs.read_to_string("OPENAI_KEY")
                    }
                    body:{
                        model: "gpt-4o"
                        max_tokens: 1000
                        stream: true
                        messages: [{content:message,role:"user"}]
                    }.to_json()
                }
                let total = ""
                net.http_request_stream(req) do net.HttpEvents{
                    on_stream: |res|{
                        let o = res.body.parse_json();
                        let s = ok{o.data.choices[0].delta.content}
                        if s total += s
                        ~total
                    }
                    on_error: |e| ~e
                }
            }
            
            send_local("Say hi");
            
            
        };
        cx.eval(code);
    }
}

impl App {
}

impl MatchEvent for App {
    
    fn handle_actions(&mut self, _cx: &mut Cx, _actions:&Actions){
    }
        
    fn handle_network_responses(&mut self, _cx: &mut Cx, _responses:&NetworkResponsesEvent ){
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
