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
    
    let comfy_ip = "10.0.0.123:8000"
    let openai_base = "http://127.0.0.1:8080";
    let Display = {mac:"", ip:"", width:0, height:0}.freeze_api()
    let displays = [
        Display{mac:"28-07-08-2c-d9-42" ip:"10.0.0.122", width:1920, height:1080},
        Display{mac:"B0-f2-f6-60-f6-e1" ip:"10.0.0.132", width:1920, height:1080},
        Display{mac:"04-E4-B6-F4-5A-8E" ip:"10.0.0.133", width:1080, height:1920}
    ] 
    let current_display = displays[0];    
              
    fn openai_chat(messages, cb){
        let req = net.HttpRequest{
            url: openai_base + "/v1/chat/completions"
            method: net.HttpMethod.POST
            headers:{"Content-Type": "application/json"}
            is_streaming: true,
            body:{
                max_tokens: 1000
                stream: true
                messages
            }.to_json()
        }
        let total = ""
        net.http_request(req) do net.HttpEvents{
            on_stream:fn(res){
                for split in res.body.to_string().split("\n\n"){
                    let o = split.parse_json();
                    ok{
                        total += o.data.choices[0].delta.content
                    }
                }
            }
            on_complete: || cb(total.trim())
            on_error: |e| ~e
        }
    }

    fn download_image(image){
        let req = net.HttpRequest{
            url: "http://" + comfy_ip + "/view?"+
            "filename=" + image.filename+
            "&subfolder=" + image.subfolder+
            "&type=" + image.type
            method: net.HttpMethod.GET
        }
        net.http_request(req) do net.HttpEvents{
            on_response: |res|{
                fs.write("/Users/admin/makepad/makepad/local/eink.png", res.body)
                upload_image(current_display);
            } 
            on_error: |e| ~e
        }
    }
                
    fn comfy_history(prompt_id, cb){
        let req = net.HttpRequest{
            url: "http://"+comfy_ip+"/history/"+prompt_id
            method: net.HttpMethod.GET
        }
        net.http_request(req) do net.HttpEvents{
            on_response: |res| cb(ok{res.body.parse_json()})
            on_error: |e| ~e
        }
    }
    
    fn connect_comfy_websocket(){
        net.web_socket("ws://"+comfy_ip+"/ws?clientId=1234") do net.WebSocketEvents{
            on_string:fn(str){
                let str = str.parse_json()
                ok{
                    if str.data.nodes["31"].state == "running" std.println(str.data.nodes["31"].value+" ")
                }
                if ok{str.data.nodes["9"].state == "finished"}{
                    std.println("Done");
                    let prompt_id = str.data.nodes["9"].prompt_id;
                    comfy_history(prompt_id) do |data|{
                        for image in data[prompt_id].outputs["9"].images{
                            download_image(image)
                        }
                    }
                }
            }
        };
    }
            
    fn comfy_post(prompt, cb){
        std.print("Rendering AI: ");
        let flow = fs.read("./examples/comfyui/flux_dev.json").parse_json()
        flow["6"].inputs.text = prompt
        flow["31"].inputs.seed = std.random_u32()
        flow["27"].inputs.width = current_display.width
        flow["27"].inputs.height = current_display.height
        let req = net.HttpRequest{
            url: "http://"+comfy_ip+"/prompt"
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
            
    fn upload_image(display){
        std.println("Uploading to " + display.ip)
        run.child(run.ChildCmd{
            cmd:"node"
            args: [
                "/usr/local/lib/node_modules/@weejewel/samsung-emdx/bin/index.mjs" "show-image"
                "--mac" display.mac
                "--host" display.ip
                "--pin" "123456"
                "--image" "/Users/admin/makepad/makepad/local/eink.png"
            ]
        }) do run.ChildEvents{
            on_stdout: |s| {}
            on_stderr: |s| ~s
        }
    }
                        
    
    // main application flow
                    
    std.random_seed();
    
    let display_iter = 0;
    let messages = [];
    fn post(){ 
        current_display = displays[display_iter % displays.len()]
        display_iter += 1
        let prompt = fs.read("./local/prompt.txt").parse_json();
        if messages.len() > 50 messages.clear()
        if prompt.clear || messages.len() == 0{
            messages.clear();
            messages.push({content:prompt.system.trim(),role:"user"});
            messages.push({content:prompt.prompt.trim(),role:"user"});
        }
        else{
            messages.push({content:prompt.next.trim(),role:"user"});
        }
        openai_chat(messages) do |res|{
            std.println("Sending prompt "+res);
            messages.push({content:res,role:"assistant"});
            comfy_post(res) do |e| ~"Prompt ID"+e
        }
    }
    
    connect_comfy_websocket();    
                
    std.start_interval(60) do fn{
        post()
    }
    post()
    
    /*
    openai_chat("Imagine a single ONLY ONE very short image prompt, about a superhero cartoon") do |res|{
        ~"prompting: "+res
        comfy_post(res) do |id|{
                                                    
        }
    };*/
                            
            
    
            
    //comfy_post("Monster police car") do |e| ~"Prompt ID"+e
};
//println!("{}", code.code);
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
