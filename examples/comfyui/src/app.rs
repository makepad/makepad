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
                
    fn openai_completion(messages){
        let task = std.task()
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
            on_complete: || task.end(total.trim())
            on_error: |e| ~e
        }
        task
    }
            
    fn comfy_image_download(image){
        let task = std.task()
        let req = net.HttpRequest{
            url: "http://" + comfy_ip + "/view?"+
                 "filename=" + image.filename+
                 "&subfolder=" + image.subfolder+
                 "&type=" + image.type
            method: net.HttpMethod.GET
        }
        net.http_request(req) do net.HttpEvents{
            on_response: |res| task.end(res.body)
            on_error: |e| ~e
        }
        task
    }
                            
    fn comfy_last_image(prompt_id){
        let task = std.task()
        let req = net.HttpRequest{
            url: "http://"+comfy_ip+"/history/"+prompt_id
            method: net.HttpMethod.GET
        }
        net.http_request(req) do net.HttpEvents{
            on_response: |res| {
                let data = res.body.parse_json()
                let image = ok{data[prompt_id].outputs["9"].images[0]}
                task.end(image)
            }
            on_error: |e| ~e
        }
        task
    }
                
    fn connect_comfy_websocket(){
        let task = std.task()
        net.web_socket("ws://"+comfy_ip+"/ws?clientId=1234") do net.WebSocketEvents{
            on_string:fn(str){
                let str = str.parse_json()
                if ok{str.data.nodes["31"].state == "running"}
                    task.emit(@progress str.data.nodes["31"].value)
                if ok{str.data.nodes["9"].state == "finished"}{
                    let prompt_id = str.data.nodes["9"].prompt_id;
                    task.emit(@done, prompt_id)
                }
            }
        };
        task
    }
                        
    fn comfy_render(prompt, display){
        let task = std.task()
        std.print("Rendering AI: ");
        let flow = fs.read("./examples/comfyui/flux_dev.json").parse_json()
        flow["6"].inputs.text = prompt
        flow["31"].inputs.seed = std.random_u32()
        flow["27"].inputs.width = display.width
        flow["27"].inputs.height = display.height
        let req = net.HttpRequest{
            url: "http://" + comfy_ip + "/prompt"
            method: net.HttpMethod.POST
            body:{prompt:flow client_id:1234}.to_json()
        }
        net.http_request(req) do net.HttpEvents{
            on_response: |res| task.end(ok{res.body.parse_json().prompt_id}:string)
        }
        task
    }
                        
    fn eink_upload_image(display, path){
        let task = std.task()
        run.child(run.ChildCmd{
            cmd: "node"
            args: [
                "/usr/local/lib/node_modules/@weejewel/samsung-emdx/bin/index.mjs" "show-image"
                "--mac" display.mac
                "--host" display.ip
                "--pin" "123456"
                "--image" path
            ]
        }) do run.ChildEvents{
            on_stdout: |s| {}
            on_stderr: |s| ~s
            on_term: || task.end()
        }
        task
    }
                
    // main application flow
                
    std.random_seed()
                
    let web_socket = connect_comfy_websocket()
                
    let display_iter = 0
    let messages = []
                
    fn post(){ 
        // handle AI prompt messages
             
        let prompt = fs.read("/Users/admin/makepad/makepad/local/prompt.txt").parse_json();
        if messages.len() > 150 messages.clear()
        if prompt.clear || messages.len() == 0{
            messages.clear()
            messages.push({content:prompt.system.trim() role:"user"})
            messages.push({content:prompt.prompt.trim() role:"user"})
        }
        else{
            messages.push({content:prompt.prompt.trim() role:"user"})
        }
        display_iter += 1
        // rotate displays
        let display = displays[display_iter % displays.len()]
                                
        let image_prompt = openai_completion(messages).last()
        
        // put the answer back in the messages array
        messages.push({content:image_prompt role:"assistant"})
                
        // flush the websocket queue
        web_socket.queue.clear()
        
        std.println("Rendering prompt:"+image_prompt)
        let prompt_id = comfy_render(image_prompt display).last()
        // this loop needs some more features like match or a for loop with array destructuring'
        
        loop{
            let d = web_socket.next();
            if d[0] == @progress std.println("Progress: "+d[1])
            if d[0] == @done {
                prompt_id = d[1];break
            }
        }
        std.println("Fetching last image from comfy");
        let image = comfy_last_image(prompt_id).last()
        // fetch the image from comfy
        let data = comfy_image_download(image).last()
        let path = "/Users/admin/makepad/makepad/local/eink.png"
        fs.write(path data)
        
        std.println("Uploading to "+display.ip)
        eink_upload_image(display path).last()
        std.println("DONE!")
    }
                
    std.start_interval(60) do fn{
        post()
    }
    post()
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
