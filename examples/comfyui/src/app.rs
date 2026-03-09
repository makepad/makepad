use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.std
    use mod.net
    use mod.fs
    use mod.edmx
    
    let self_ip = "10.0.0.112"
    let comfy_ip = "10.0.0.165:8000"
    let openai_base = "http://127.0.0.1:8080"
    let prompt_path = "/Users/admin/prompt.txt"
    let auto_seconds = 60
    let Display = {mac:"" ip:"" landscape:false prompt:"empty"}.freeze_api()
    let displays = [
        Display{mac:"04-E4-B6-F4-5A-8E" ip:"10.0.0.182" landscape:false}
        Display{mac:"28:07:08:2C:D9:42" ip:"10.0.0.198" landscape:true}
        Display{mac:"B0-f2-f6-60-f6-e1" ip:"10.0.0.204" landscape:true}
        Display{mac:"04:E4:B6:F4:1D:DC" ip:"10.0.0.124" landscape:true}
    ]

    let models = {
        flux:{
            file: "./examples/comfyui/flux_dev_full_text_to_image.json"
            sampler: "31"
            image: "27"
            prompt: "41"
            save: "9"
            width: 1600
            height: 896
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

    let model = models.flux
    let display_iter = 0
    let is_running = false
    let auto_enabled = false
    let auto_timer = nil
    let messages = []
    let current_content_json = "{}"
    let current_image_data = []

    fn ui_log(line){
        let old = ui.log_view.text()
        if old == "" ui.log_view.set_text(line)
        else ui.log_view.set_text(old + "\n" + line)
    }

    fn set_status(text){
        ui.status_value.set_text(text)
        ui_log(text)
    }

    fn set_progress(text){
        ui.progress_value.set_text(text)
    }

    fn set_display(text){
        ui.display_value.set_text(text)
    }

    fn set_last_prompt(text){
        ui.last_prompt_view.set_text(text)
    }

    fn set_preview_path(text){
        ui.preview_path_value.set_text(text)
    }

    fn set_preview_image(bytes){
        ui.preview_image.load_image_from_data_async(bytes)
        set_preview_path("binary data (" + bytes.len() + " bytes)")
    }

    fn refresh_auto_button(){
        if auto_enabled ui.toggle_auto_btn.set_text("Pause Auto")
        else ui.toggle_auto_btn.set_text("Resume Auto")
    }

    fn default_prompt_json(){
        "{\n  \"system\": \"You are an image prompt assistant.\",\n  \"prompt\": \"\",\n  \"clear\": false\n}"
    }

    fn load_prompt_into_ui(){
        let prompt_text = ok{fs.read(prompt_path)}
        if prompt_text == nil prompt_text = default_prompt_json()
        ui.prompt_input.set_text(prompt_text)
        set_status("Prompt loaded from prompt.txt")
    }

    fn save_prompt_from_ui(){
        let prompt_text = ui.prompt_input.text()
        fs.write(prompt_path prompt_text)
        set_status("Prompt saved to prompt.txt")
    }

    fn start_auto_loop(){
        if auto_enabled return
        auto_enabled = true
        auto_timer = std.start_interval(auto_seconds) do fn{
            post()
        }
        refresh_auto_button()
        set_status("Auto loop started")
    }

    fn stop_auto_loop(){
        if !auto_enabled return
        auto_enabled = false
        if auto_timer != nil std.stop_timer(auto_timer)
        auto_timer = nil
        refresh_auto_button()
        set_status("Auto loop paused")
    }

    fn sleep_seconds(seconds){
        let promise = std.promise()
        std.start_timeout(seconds, || promise.resolve(true))
        promise.await()
    }

    fn openai_completion(messages){
        let promise = std.promise()
        let req = net.HttpRequest{
            url: openai_base + "/v1/chat/completions"
            method: net.HttpMethod.POST
            headers:{"Content-Type": "application/json"}
            is_streaming: true
            body:{
                max_tokens: 1000
                stream: true
                messages
            }.to_json()
        }
        net.http_request(req) do net.HttpEvents{
            let total = ""
            on_stream:fn(res){
                for split in res.body.to_string().split("\n\n"){
                    let o = split.parse_json()
                    ok{
                        total += o.data.choices[0].delta.content
                        set_last_prompt(total)
                    }
                }
            }
            on_complete: || {
                promise.resolve(total.trim())
            }
            on_error: |e| {
                set_status("OpenAI completion error")
                ~e
                promise.resolve("")
            }
        }
        promise
    }

    fn comfy_image_download(image){
        let promise = std.promise()
        let filename = ("" + image.filename).url_encode()
        let subfolder = ("" + image.subfolder).url_encode()
        let image_type = ("" + image.type).url_encode()
        let req = net.HttpRequest{
            url: "http://" + comfy_ip + "/view?"+
            "filename=" + filename+
            "&subfolder=" + subfolder+
            "&type=" + image_type
            method: net.HttpMethod.GET
        }
        net.http_request(req) do net.HttpEvents{
            on_response: |res| promise.resolve(res.body)
            on_error: |e| {
                set_status("Comfy image download error")
                ~e
                promise.resolve(nil)
            }
        }
        promise
    }

    fn comfy_last_image(prompt_id, model){
        let promise = std.promise()
        let req = net.HttpRequest{
            url: "http://"+comfy_ip+"/history/"+prompt_id
            method: net.HttpMethod.GET
        }
        net.http_request(req) do net.HttpEvents{
            on_response: |res| {
                let data = ok{res.body.parse_json()}
                let image = ok{data[prompt_id].outputs[model.save].images[0]}
                promise.resolve(image)
            }
            on_error: |e| {
                set_status("Comfy history error")
                ~e
                promise.resolve(nil)
            }
        }
        promise
    }

    fn connect_comfy_websocket(model){
        let task = std.task()
        net.web_socket("ws://"+comfy_ip+"/ws?clientId=8a327a3e4961419ea7386c542f0ea491") do net.WebSocketEvents{
            on_string:fn(str){
                let msg = ok{str.parse_json()}
                if msg == nil return;
                if ok{msg.type == "execution_start"}
                    task.emit(@progress ok{0.0})
                if ok{msg.type == "progress"}
                    task.emit(@progress ok{msg.data.value/msg.data.max})
                if ok{msg.data.nodes[model.save].state == "finished"}{
                    task.emit(@progress ok{1.0})
                    let prompt_id = ok{msg.data.nodes[model.save].prompt_id}
                    task.emit(@done, prompt_id)
                }
            }
            on_error:fn(e){
                set_status("Comfy websocket error")
                ~e
                task.emit(@error, e)
            }
            on_closed:fn(){
                task.emit(@closed)
            }
        }
        task
    }

    fn comfy_render(prompt, display, model){
        let promise = std.promise()
        let flow = fs.read(model.file).parse_json()

        let prompt_style = ok{prompt.style_and_keywords}
        if prompt_style == nil prompt_style = ""
        let prompt_visual = ok{prompt.visual_description}
        if prompt_visual == nil prompt_visual = ""

        flow[model.prompt].inputs.clip_l = prompt_style
        flow[model.prompt].inputs.t5xxl = prompt_visual

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
            on_response: |res| promise.resolve(ok{res.body.parse_json().prompt_id})
            on_error: |e| {
                set_status("Comfy render request error")
                ui_log("Comfy /prompt network error: " + ("" + e))
                ~e
                promise.resolve(nil)
            }
        }
        promise
    }

    fn http_response(headers, displays){
        if headers.path == "/content.json"{
            std.println("Content.json loaded")
            let body = current_content_json
            return net.HttpServerResponse{
                header:
                    "HTTP/1.1 200 OK\r\n" +
                    "Content-Type: application/json; charset=utf-8\r\n" +
                    "Content-Length: " + body.len() + "\r\n" +
                    "Connection: close\r\n\r\n"
                body: body
            }
        }
        if headers.path == "/image"{
            if current_image_data.len() > 0{
                std.println("Uploading image")
                let body = current_image_data
                return net.HttpServerResponse{
                    header:
                        "HTTP/1.1 200 OK\r\n" +
                        "Content-Type: image/png\r\n" +
                        "Accept-Ranges: bytes\r\n" +
                        "Cache-Control: public, max-age=0\r\n" +
                        "Content-Length: " + body.len() + "\r\n" +
                        "Connection: close\r\n\r\n"
                    body: body
                }
            }
            let body = "No image"
            return net.HttpServerResponse{
                header:
                    "HTTP/1.1 404 Not Found\r\n" +
                    "Content-Type: text/plain; charset=utf-8\r\n" +
                    "Content-Length: " + body.len() + "\r\n" +
                    "Connection: close\r\n\r\n"
                body: body
            }
        }

        let idx = headers.search.to_f64()
        let body =
            if idx.is_number()
                displays[idx].prompt
            else
                mod.edmx.http_body
        net.HttpServerResponse{
            header:
                "HTTP/1.1 200 OK\r\n" +
                "Content-Type: text/html; charset=utf-8\r\n" +
                "Content-Length: " + body.len() + "\r\n" +
                "Connection: close\r\n\r\n"
            body: body
        }
    }

    let http_server = net.http_server(net.HttpServerOptions{
        listen:"0.0.0.0:3000"
    }, net.HttpServerEvents{
        on_get: |headers| http_response(headers, displays)
    })

    let comfy_socket = connect_comfy_websocket(model)

    fn post(){
        if is_running {
            ui_log("Run ignored: already running")
            return false
        }

        is_running = true
        ui.run_now_btn.set_text("Running...")

        let prompt_source = ui.prompt_input.text()
        let prompt = ok{prompt_source.parse_json()}
        if prompt == nil {
            set_status("Invalid prompt JSON in editor")
            is_running = false
            ui.run_now_btn.set_text("Run Now")
            return false
        }

        fs.write(prompt_path prompt_source)

        let system_text = ok{prompt.system}
        if system_text == nil system_text = ""
        else system_text = system_text.trim()

        let user_text = ok{prompt.prompt}
        if user_text == nil {
            set_status("Missing prompt.prompt in prompt JSON")
            is_running = false
            ui.run_now_btn.set_text("Run Now")
            return false
        }
        user_text = ("" + user_text).trim()
        if user_text == "" {
            set_status("prompt.prompt is empty")
            is_running = false
            ui.run_now_btn.set_text("Run Now")
            return false
        }

        let clear_prompt = ok{prompt.clear} == true

        if messages.len() > 40 messages.clear()
        if clear_prompt || messages.len() == 0{
            messages.clear()
            messages.push({content:system_text role:"user"})
            messages.push({content:user_text role:"user"})
        }
        else{
            messages[0] = {content:system_text role:"user"}
            messages.push({content:user_text role:"user"})
        }

        let display = displays[display_iter % displays.len()]
        display_iter += 1
        set_display(display.ip + "  " + display.mac)

        set_status("Generating image prompt")
        let image_prompt_text = openai_completion(messages).await()
        let image_prompt = image_prompt_text
            .strip_prefix("```json")
            .strip_suffix("```")
            .parse_json()
        if image_prompt == nil{
            set_status("AI did not return JSON prompt")
            ui_log("AI raw response:\n" + image_prompt_text)
            is_running = false
            ui.run_now_btn.set_text("Run Now")
            return false
        }

        messages.push({content:image_prompt_text role:"assistant"})
        comfy_socket.queue.clear()

        let visual_description = ok{image_prompt.visual_description}
        let style_keywords = ok{image_prompt.style_and_keywords}
        set_last_prompt(visual_description + "\n" + style_keywords)

        set_status("Submitting job to ComfyUI")
        let prompt_id = comfy_render(image_prompt display model).await()
        if prompt_id == nil {
            set_status("Comfy render request failed")
            is_running = false
            ui.run_now_btn.set_text("Run Now")
            return false
        }

        set_status("Rendering in ComfyUI")
        let event_prompt_id = prompt_id
        loop{
            let event = comfy_socket.next()
            if event == nil {
                set_status("Comfy websocket closed")
                is_running = false
                ui.run_now_btn.set_text("Run Now")
                return false
            }
            if event[0] == @progress{
                set_progress("" + (event[1] * 100) + "%")
            }
            if event[0] == @done{
                event_prompt_id = event[1]
                break
            }
            if event[0] == @error || event[0] == @closed{
                set_status("Comfy websocket error")
                is_running = false
                ui.run_now_btn.set_text("Run Now")
                return false
            }
        }
        
        set_status("Fetching image from ComfyUI")
        let image = comfy_last_image(event_prompt_id, model).await()
        if image == nil {
            set_status("Comfy history returned no image")
            is_running = false
            ui.run_now_btn.set_text("Run Now")
            return false
        }
        if ok{image.filename} == nil || ok{image.subfolder} == nil || ok{image.type} == nil{
            set_status("Comfy history returned invalid image payload")
            is_running = false
            ui.run_now_btn.set_text("Run Now")
            return false
        }

        set_status("Downloading image")
        let data = comfy_image_download(image).await()
        if data == nil {
            set_status("Failed to download ComfyUI image")
            is_running = false
            ui.run_now_btn.set_text("Run Now")
            return false
        }
        
        set_status("Uploading to EMDX " + display.ip)
        current_image_data = data
        set_preview_image(current_image_data)
        let file_id = "EDMX" + std.random_u32() + std.random_u32()
        current_content_json = mod.edmx.build_content_json(self_ip, "3000", file_id, current_image_data.len())
        let content_url = "http://" + self_ip + ":3000/content.json"
        let upload_result = edmx.upload_image(display, content_url, sleep_seconds)
        if upload_result == nil || ok{upload_result.is_ok} != true {
            let err = ok{upload_result.error}
            if err == nil err = "EDMX upload failed"
            set_status("" + err)
            is_running = false
            ui.run_now_btn.set_text("Run Now")
            return false
        }

        display.prompt = visual_description + " - " + style_keywords

        set_status("Done")
        is_running = false
        ui.run_now_btn.set_text("Run Now")
        true
    }

    let app = startup() do #(App::script_component(vm)){
        ui: Root{
            on_startup: ||{
                let _keep_http_alive = http_server
                set_status("Starting ComfyUI dashboard")
                set_display("-")
                set_last_prompt("")
                set_preview_path("-")
                refresh_auto_button()
                load_prompt_into_ui()
                start_auto_loop()
                post()
            }

            main_window := Window{
                window.inner_size: vec2(980, 760)
                body +: {
                    width: Fill
                    height: Fill
                    flow: Down
                    spacing: 10
                    padding: 12

                    RoundedView{
                        width: Fill
                        height: Fit
                        padding: 14
                        draw_bg.color: #223344
                        draw_bg.radius: 8.0
                        flow: Down
                        spacing: 6

                        Label{text: "ComfyUI + EMDX" draw_text.color: #fff draw_text.text_style.font_size: 14}
                        Label{text: "Edit prompt.json text, run now, or leave auto loop enabled." draw_text.color: #c8d0d8 draw_text.text_style.font_size: 10}
                    }

                    View{
                        width: Fill
                        height: Fill
                        flow: Right
                        spacing: 10

                        View{
                            width: Fill
                            height: Fill
                            flow: Down
                            spacing: 10

                            View{
                                width: Fill
                                height: Fit
                                flow: Right
                                spacing: 8

                                load_prompt_btn := ButtonFlat{
                                    text: "Load prompt.txt"
                                    on_click: || load_prompt_into_ui()
                                }
                                save_prompt_btn := ButtonFlat{
                                    text: "Save prompt.txt"
                                    on_click: || save_prompt_from_ui()
                                }
                                run_now_btn := Button{
                                    text: "Run Now"
                                    on_click: || post()
                                }
                                toggle_auto_btn := ButtonFlat{
                                    text: "Pause Auto"
                                    on_click: || {
                                        if auto_enabled stop_auto_loop()
                                        else start_auto_loop()
                                    }
                                }
                            }

                            prompt_input := TextInput{
                                width: Fill
                                height: 140
                                is_multiline: true
                                empty_text: "{ \"system\": \"...\", \"prompt\": \"...\", \"clear\": false }"
                            }

                            RoundedView{
                                width: Fill
                                height: Fit
                                padding: 10
                                draw_bg.color: #1f232b
                                draw_bg.radius: 6.0
                                flow: Down
                                spacing: 6

                                View{width: Fill height: Fit flow: Right spacing: 8
                                    Label{text: "Status:" draw_text.color: #9fb3c8 draw_text.text_style.font_size: 10}
                                    status_value := Label{text: "Idle" draw_text.color: #ffffff draw_text.text_style.font_size: 10}
                                }
                                View{width: Fill height: Fit flow: Right spacing: 8
                                    Label{text: "Progress:" draw_text.color: #9fb3c8 draw_text.text_style.font_size: 10}
                                    progress_value := Label{text: "-" draw_text.color: #ffffff draw_text.text_style.font_size: 10}
                                }
                                View{width: Fill height: Fit flow: Right spacing: 8
                                    Label{text: "Display:" draw_text.color: #9fb3c8 draw_text.text_style.font_size: 10}
                                    display_value := Label{text: "-" draw_text.color: #ffffff draw_text.text_style.font_size: 10}
                                }
                            }

                            Label{text: "Last prompt sent" draw_text.color: #c8d0d8 draw_text.text_style.font_size: 10}
                            last_prompt_view := TextInput{
                                width: Fill
                                height: 56
                                is_multiline: true
                                is_read_only: true
                                empty_text: "No prompt yet"
                            }

                            Label{text: "Activity log" draw_text.color: #c8d0d8 draw_text.text_style.font_size: 10}
                            log_scroller := ScrollYView{
                                width: Fill
                                height: Fill
                                show_bg: false
                                log_view := TextInput{
                                    width: Fill
                                    height: Fit
                                    is_multiline: true
                                    is_read_only: true
                                    empty_text: "Logs will appear here"
                                }
                            }
                        }

                        View{
                            width: 320
                            height: Fill
                            flow: Down
                            spacing: 8

                            Label{text: "Current uploaded image" draw_text.color: #c8d0d8 draw_text.text_style.font_size: 10}
                            RoundedView{
                                width: Fill
                                height: Fill
                                padding: 10
                                draw_bg.color: #1f232b
                                draw_bg.radius: 6.0
                                flow: Down
                                spacing: 6

                                preview_image := Image{
                                    width: Fill
                                    height: Fill
                                    fit: ImageFit.Smallest
                                }
                                View{width: Fill height: Fit flow: Right spacing: 8
                                    Label{text: "File:" draw_text.color: #9fb3c8 draw_text.text_style.font_size: 10}
                                    preview_path_value := Label{text: "-" draw_text.color: #ffffff draw_text.text_style.font_size: 10}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    app
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl MatchEvent for App {
    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        crate::edmx::register_socket_extensions(vm);
        crate::edmx::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
