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
#[derive(Live)]
pub struct App {
    #[live] ui: WidgetRef,
}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
}
}

impl LiveHook for App {
    fn after_new_from_doc(&mut self, cx:&mut Cx){
                                
        let code = script!{
            use mod.net
            use mod.fs
            use mod.std
            use mod.run
            
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
                            ok{
                                total += o.data.choices[0].delta.content
                                ~total
                            }
                        }
                    }
                    on_complete: || cb(total)
                    on_error: |e| ~e
                }
            }
            /*
            openai_chat("Imagine a single ONLY ONE very short image prompt, about a superhero cartoon") do |res|{
                ~"prompting: "+res
                comfy_post(res) do |id|{
                                
                }
            };*/
                
                
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
                
            net2.web_socket("ws://10.0.0.123:8000/ws?clientId=1234") do net.WebSocketEvents{
                on_string:fn(str){
                    let str = str.parse_json()
                    ok{
                        if str.data.nodes["31"].state == "running" progress(str.data.nodes["31"].value)
                    }
                    if ok{str.data.nodes["9"].state == "finished"}{
                        finish_image(str.data.nodes["9"].prompt_id)
                    }
                }
            };
                
            fn comfy_post(prompt, cb){
                ~"COMFY POST"
                let flow = fs.read("./examples/comfyui/flux_schnell.json").parse_json()
                flow["6"].inputs.text = prompt
                flow["31"].inputs.seed = 123421127583743362
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
            
            run.child(run.ChildCmd{cmd:"ls"}) do run.ChildEvents{
                on_stdout: |s| ~s
            }
            std.random_seed();
            std.start_interval(0.5) do |s| ~std.random_u32()
                
            // comfy_post("Soundwave waveform rendering art of a black hole bright white ultra bright") do |e| ~"Prompt ID"+e
        };
        cx.eval(code);
    }
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
