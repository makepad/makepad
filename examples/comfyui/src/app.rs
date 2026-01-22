use makepad_draw2::*;

app_main!(App); 
script_mod!{
    use mod.std.*;
    #(App::script_api(vm)){
    }
}

impl App{
    fn run(vm:&mut ScriptVm)->Self{
        crate::makepad_draw2::script_mod(vm);
        let r = App::from_script_mod(vm, script_mod);
        r
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[script] window: WindowHandle,
    #[script] pass: DrawPass,
    #[script] depth_texture: Texture,
   // #[script] draw_quad: DrawQuad,
    #[script] main_draw_list: DrawList2d,
}
 
impl MatchEvent for App{
    fn handle_startup(&mut self, cx:&mut Cx){
        let code = script!{
            use mod.net
            use mod.fs
            use mod.std
            use mod.run
            let self_ip = "10.0.0.112"
            let comfy_ip = "10.0.0.165:8000"
            let openai_base = "http://127.0.0.1:8080"
            let Display = {mac:"" ip:"" landscape:false prompt:"empty"}.freeze_api()
            let displays = [
                Display{mac:"04-E4-B6-F4-5A-8E" ip:"10.0.0.182" landscape:false} // left
                Display{mac:"28:07:08:2C:D9:42" ip:"10.0.0.198" landscape:true} // table
                Display{mac:"B0-f2-f6-60-f6-e1" ip:"10.0.0.204" landscape:true} // door
                Display{mac:"04:E4:B6:F4:1D:DC" ip:"10.0.0.124" landscape:true} // side
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
                                        
            fn comfy_last_image(prompt_id, model){
                let task = std.task()
                let req = net.HttpRequest{
                    url: "http://"+comfy_ip+"/history/"+prompt_id
                    method: net.HttpMethod.GET
                }
                net.http_request(req) do net.HttpEvents{
                    on_response: |res| {
                        let data = res.body.parse_json()
                        let image = ok{data[prompt_id].outputs[model.save].images[0]}
                        task.end(image)
                    }
                    on_error: |e| ~e
                }
                task
            }
                
            let models = {
                flux:{
                    file: "./examples/comfyui/flux_dev_full_text_to_image.json"
                    sampler: "31"
                    image: "27"
                    prompt: "41"
                    save: "9"
                    width: 1600
                    height: 900
                }
                qwen:{
                    file: "./examples/comfyui/image_qwen_image.json"
                    sampler: "3"
                    image: "58"
                    prompt: "6"
                    save:"60"
                    width: 1664
                    height: 928
                }
            }
                
            fn connect_comfy_websocket(model){
                let task = std.task()
                net.web_socket("ws://"+comfy_ip+"/ws?clientId=8a327a3e4961419ea7386c542f0ea491") do net.WebSocketEvents{
                    on_string:fn(str){
                        let str = str.parse_json()
                        if ok{str.data.nodes[model.sampler].state == "running"}
                        task.emit(@progress str.data.nodes[model.sampler].value)
                        if ok{str.data.nodes[model.save].state == "finished"}{
                            let prompt_id = str.data.nodes[model.save].prompt_id;
                            task.emit(@done, prompt_id)
                        }
                    }
                    on_error:fn(e){
                        std.println(e)
                    }
                };
                task
            }
                                    
            fn comfy_render(prompt, display, model){
                let task = std.task()
                std.println("Rendering AI: ");
                let flow = fs.read(model.file).parse_json()
                        
                flow[model.prompt].inputs.clip_l = prompt.style_and_keywords
                flow[model.prompt].inputs.t5xxl = prompt.visual_description
                        
                flow[model.sampler].inputs.seed = std.random_u32()
                flow[model.image].inputs.width = 
                if display.landscape model.width else model.height
                flow[model.image].inputs.height = 
                if display.landscape model.height else model.width
                
                let req = net.HttpRequest{
                    url: "http://" + comfy_ip + "/prompt"
                    method: net.HttpMethod.POST
                    body:{prompt:flow client_id:"8a327a3e4961419ea7386c542f0ea491"}.to_json()
                }
                net.http_request(req) do net.HttpEvents{
                    on_response: |res| task.end(ok{res.body.parse_json().prompt_id})
                }
                task
            }
                                    
            fn eink_upload_image(display, path){
                let task = std.task()
                std.println("Uploading image: "+display.mac+" "+display.ip+" "+path)
                run.child(run.ChildCmd{
                    cmd: "node"
                    args: [
                        "/usr/local/lib/node_modules/@weejewel/samsung-emdx/bin/index.mjs" "show-image"
                        "--mac" display.mac
                        "--host" display.ip
                        "--local-ip" self_ip
                        "--pin" "123456"
                        "--image" path
                    ]
                }) do run.ChildEvents{
                    on_stdout: |s| {}
                    on_stderr: |s| std.println(s)
                    on_term: || task.end()
                }
                task
            }
                            
            // main application flow
                            
            std.random_seed()
                    
            let model = models.flux;
                                
            let web_socket = connect_comfy_websocket(model)
                            
            let display_iter = 0
            let messages = []
            
            let http_body = "
            <body onclick='document.documentElement.requestFullscreen()' ondblclick='location.reload()' style='margin:0;padding:20;background:#fff;color:#000;display:flex;height:100vh;overflow:hidden'>
            <b id='d' style='font:5vw sans-serif'></b>
            <script>
            u = location.origin + location.pathname + '?' + location.pathname.slice(1);
            f = () => {
                fetch(u)
                .then(r => r.ok ? r.text() : null)
                .then(t => { if (t !== null) d.innerText = t })
                .catch(e => 0)
                .finally(() => setTimeout(f, 1000));
            };
            f();
            </script>
            </body>
            "
            
            let http_server = net.http_server(net.HttpServerOptions{
                listen:"0.0.0.0:8081"
            }, net.HttpServerEvents{
                on_get: |headers|{
                    let idx = headers.search.to_f64()
                    net.HttpServerResponse{
                        header:"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n"
                        body: if idx.is_number()
                            displays[idx].prompt
                        else
                            http_body
                    }
                    
                }
            })
            
                        
            fn post(){ 
                // handle AI prompt messages
                        
                let prompt = fs.read("/Users/admin/prompt.txt").parse_json();
                 
                if messages.len() > 40 messages.clear()
                if prompt.clear || messages.len() == 0{
                    messages.clear()
                    messages.push({content:prompt.system.trim() role:"user"})
                    messages.push({content:prompt.prompt.trim() role:"user"})
                }
                else{
                    messages[0] = {content:prompt.system.trim() role:"user"}
                    messages.push({content:prompt.prompt.trim() role:"user"})
                }
                // rotate displays
                                           
                let display = displays[display_iter % displays.len()]
                display_iter += 1
                let image_prompt = openai_completion(messages).last()
                /*
                let image_prompt = {
                    visual_description:prompt.prompt,
                    style_and_keywords:prompt.prompt
                }*/
                
                // put the answer back in the messages array
                messages.push({content:image_prompt role:"assistant"})
                                
                // flush the websocket queue
                web_socket.queue.clear()
                
                let image_prompt = image_prompt.strip_prefix("```json").strip_suffix("```").parse_json();
                        
                std.println("Rendering prompt: "+image_prompt.visual_description+" keywords: "+image_prompt.style_and_keywords)
                        
                let prompt_id = comfy_render(image_prompt display model).last()
                // this loop needs some more features like match or a for loop with array destructuring'
                loop{
                    let d = web_socket.next();
                    if d[0] == @progress std.println("Progress: "+d[1])
                    if d[0] == @done {
                        prompt_id = d[1];break
                    }
                }
                std.println("Fetching last image from comfy");
                let image = comfy_last_image(prompt_id, model).last()
                // fetch the image from comfy
                let data = comfy_image_download(image).last()
                let path = "/Users/admin/makepad/makepad/local/eink.png"
                fs.write(path data)
                        
                std.println("Uploading to " + display.ip)
                eink_upload_image(display path).last()
                let set_prompt = image_prompt.visual_description + " - " + image_prompt.style_and_keywords
                let set_display = display
                std.println("DONE!")
                std.start_timeout(17, || set_display.prompt = set_prompt)
            }
                            
            std.start_interval(60) do fn{
                post()
            }
            post()
        };
        cx.eval(code);
        
        
        self.window.set_pass(cx, &self.pass);
        self.depth_texture = Texture::new_with_format(cx, TextureFormat::DepthD32{
            size: TextureSize::Auto,
            initial: true,
        });
        self.pass.set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));
        self.pass.set_window_clear_color(cx, vec4(0.0, 0.0, 1.0, 0.0));
    }
    
    fn handle_draw_2d(&mut self, cx: &mut Cx2d){
        if !cx.will_redraw(&mut self.main_draw_list, Walk::default()) {
            return
        }
        
        cx.begin_pass(&self.pass, None);
        self.main_draw_list.begin_always(cx);
        
        let size = cx.current_pass_size();
        cx.begin_root_turtle(size, Layout::flow_down());
        
        // draw things here
        
        cx.end_pass_sized_turtle();
        self.main_draw_list.end(cx);
        cx.end_pass(&self.pass);
    }
            
    fn handle_actions(&mut self, _cx: &mut Cx, _actions:&Actions){
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        let _ = self.match_event_with_draw_2d(cx, event);
    }
}
