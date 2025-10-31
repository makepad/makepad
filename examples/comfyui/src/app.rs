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
            
    fn openai_chat(message, cb){
        let req = net.HttpRequest{
            url: "http://127.0.0.1:8080/v1/chat/completions"
            method: net.HttpMethod.POST
            headers:{"Content-Type": "application/json"}
            is_streaming: true,
            body:{
                max_tokens: 1000
                stream: true
                messages: [
                    {content:message,role:"user"}
                ]
            }.to_json()
        }
        let total = ""
        net.http_request(req) do net.HttpEvents{
            on_stream:fn(res){
                for split in res.body.to_string().split("\n\n"){
                    let o = split.parse_json();
                    ok{total += o.data.choices[0].delta.content}
                }
            }
            on_complete: || cb(total)
            on_error: |e| ~e
        }
    }
    
    openai_chat("Come up with a single very short image prompt of bokeh plants") do |res|{
        ~"prompting: "+res
        comfy_post(res) do |id|{
            
        }
    };
    
    fn progress(progress){
        ~"PROGRESS:"+progress
    }    
        
    fn finish_image(prompt_id){
        comfy_history(prompt_id) do |data|{
            for image in data[prompt_id].outputs["9"].images{
                download_image(image)
            }
        }
    }
    
    fn download_image(image){
        let req = net.HttpRequest{
            url: "http://10.0.0.123:8000/view?"+
                 "filename="+image.filename+
                 "&subfolder="+image.subfolder+
                 "&type="+image.type
            method: net.HttpMethod.GET
        }
        net.http_request(req) do net.HttpEvents{
            on_response: |res|{
                fs.write("./dump.png", res.body)
            }
            on_error: |e| ~e
        }
    }
    
    fn comfy_history(prompt_id, cb){
        let req = net.HttpRequest{
            url: "http://10.0.0.123:8000/history/"+prompt_id
            method: net.HttpMethod.GET
        }
        net.http_request(req) do net.HttpEvents{
            on_response: |res| cb(ok{res.body.parse_json()})
            on_error: |e| ~e
        }
    }
    
    net.web_socket("ws://10.0.0.123:8000/ws?clientId=1234") do net.WebSocketEvents{
        on_string:fn(str){
            let str = str.parse_json()
            ok{progress(str.data.nodes["31"].value)}
            if ok{str.data.nodes["9"].state == "finished"}{
                finish_image(str.data.nodes["9"].prompt_id)
            }
        }
    };
    
    fn comfy_post(prompt, cb){
        let flow = fs.read("./examples/comfyui/flux_schnell.json").parse_json()
        flow["6"].inputs.text = prompt
        let req = net.HttpRequest{
            url: "http://10.0.0.123:8000/prompt"
            method: net.HttpMethod.POST
            body:{prompt:flow, client_id:1234}.to_json()
        }
        net.http_request(req) do net.HttpEvents{
            on_response: |res|{
                cb(ok{res.body.parse_json().prompt_id})
            }
            on_error: |e| ~e
        }
    }
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
