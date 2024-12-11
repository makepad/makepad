use crate::makepad_live_id::*;
use makepad_micro_serde::*;
use makepad_widgets::*;
use std::fs;
use std::time::{Instant};
use crate::database::*; 
use crate::comfyui::*; 
use makepad_http::server::*;
use std::net::{SocketAddr, IpAddr, Ipv4Addr};
use std::sync::mpsc;
use std::cell::RefCell;
use std::sync::{Arc,Mutex};
use crate::whisper::*;
   
live_design!{
    use link::widgets::*;
    use link::theme::*;
    use link::shaders::*;
    use crate::app_ui::AppUI;
    use crate::app_ui::AppWindow;
    
    App = {{App}} { 
        ui: <Root> {
            <Window> {
                window: {inner_size: vec2(2000, 1024)},
                caption_bar = {visible: true, caption_label = {label = {text: "GenAI"}}},
                hide_caption_on_fullscreen: true,
                body = <AppUI>{}
            }
            <Window> {
               hide_caption_on_fullscreen:true
                window: {inner_size: vec2(960, 540)},
                body = <AppWindow>{}
            }
        }
    }
}

app_main!(App);

struct Machine {
    ip: String,
    id: LiveId,
    running: MachineRunning,
    fetching: Option<(String,PromptState)>,
    web_socket: Option<WebSocket>
}
#[derive(Debug)]
enum MachineRunning{
    Stopped,
    _UploadingImage{
        photo_name: String,
        prompt_state: PromptState,
    },
    RunningPrompt {
        photo_name: String,
        prompt_state: PromptState,
    }
}

impl MachineRunning{
    fn is_running(&self)->bool{
        match self{
            Self::Stopped=>false,
            _=>true
        }
    } 
}

impl Machine {
    fn new(ip: &str, id: LiveId) -> Self {Self {
        ip: ip.to_string(),
        id,
        running: MachineRunning::Stopped,
        fetching: None,
        web_socket: None
    }}
}

 
#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
    #[rust(vec![
       Machine::new("10.0.0.124:8188", live_id!(m1)),
       Machine::new("10.0.0.107:8188", live_id!(m2)),
       //Machine::new("192.168.8.231:8188", id_lut!(m1)),
    ])] machines: Vec<Machine>,
    
    #[rust(Database::new(cx))] db: Database,
    
    #[rust] filtered: FilteredDb,
    //#[rust(10000u64)] last_seed: u64,
    
    #[rust] current_image: Option<ImageId>,
    #[rust] _current_photo_name: Option<String>,
    #[rust([Texture::new(cx)])] video_input: [Texture; 1],
    #[rust] video_recv: ToUIReceiver<(usize, VideoBuffer)>,
    #[rust] remote_screens: Arc<Mutex<RefCell<Vec<(u64, Ipv4Addr,mpsc::Sender<Vec<u8>>)>>>>,
    #[rust] llm_chat: Vec<(LLMMsg,String)>,
    #[rust] voice_input: Option<WhisperProcess>,
    #[rust] delay_timer: Timer,
}

enum LLMMsg{
    AI,
    Human,
    Progress
}

impl LiveRegister for App{
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
        crate::app_ui::live_design(cx);
    }
}
     
impl App {
    /*
    fn get_free_machine(&self)->Option<LiveId>{
        for machine in &self.machines {
            if machine.running.is_running() {
                continue
            }
            return Some(machine.id)
        }
        None
    }*/
    
    fn check_to_render(&mut self, cx: &mut Cx) {
        for machine in &mut self.machines {
            if machine.running.is_running() {
                continue
            }
            
            if !self.ui.check_box(id!((machine.id).render_check_box)).selected(cx){
                continue
            }
            
            let prompt = self.ui.text_input(id!(prompt_input)).text();
            let seed = if self.ui.check_box(id!(random_check_box)).selected(cx) {
                let seed = LiveId::from_str(&format!("{:?}", Instant::now())).0;
                self.ui.text_input(id!(seed_input)).set_text_and_redraw(cx, &format!("{}",seed));
                seed
            }
            else{ // read seed from box
                self.ui.text_input(id!(seed_input)).text().parse::<u64>().unwrap_or(0)
            };
            
            let url = format!("http://{}/prompt", machine.ip);
            let mut request = HttpRequest::new(url, HttpMethod::POST);
                        
            let model = LiveId::from_str(&self.ui.drop_down(&[machine.id, live_id!(model)]).selected_label().to_lowercase());
            self.ui.label(id!((machine.id).last_set)).set_text_and_redraw(cx, &prompt);
            let res = self.ui.drop_down(id!((machine.id).(model).resolution)).selected_label();
            let (width,height) = split_res(&res);
            
            fn split_res(label:&str)->(&str,&str){
                let mut split = label.split(" ").next().unwrap().split("x");
                let width = split.next().unwrap();
                let height = split.next().unwrap();
                (width,height)
            }
                
            request.set_header("Content-Type".to_string(), "application/json".to_string());
            
            match model{
                live_id!(flux)=>{
                    println!("FLUX");
                    let steps = self.ui.slider(id!((machine.id).flux.steps_slider)).value().unwrap_or(8.0) as usize;
                    // lets check which workspace we are in
                    let ws = fs::read_to_string("examples/genai/workspace_flux.json").unwrap();
                    let ws = ws.replace("CLIENT_ID", "1234");
                    let ws = ws.replace("PROMPT", &prompt.replace("\n", "").replace("\"", ""));
                    let ws = ws.replace("11223344", &format!("{}", seed));
                    let ws = ws.replace("\"steps\": 10", &format!("\"steps\": {}", steps));
                    let ws = ws.replace("\"width\": 1920", &format!("\"width\": {}", width));
                    let ws = ws.replace("\"height\": 1088", &format!("\"height\": {}", height));
                    request.set_body(ws.as_bytes().to_vec());
                }
                live_id!(hunyuan)=>{
                    let frames = self.ui.slider(id!((machine.id).hunyuan.frames_slider)).value().unwrap_or(9.0) as usize;
                    let ws = fs::read_to_string("examples/genai/workspace_hunyuan.json").unwrap();
                    let ws = ws.replace("CLIENT_ID", "1234");
                    let ws = ws.replace("PROMPT", &prompt.replace("\n", "").replace("\"", ""));
                    let ws = ws.replace("11223344", &format!("{}", seed));
                    let ws = ws.replace("\"num_frames\": 17", &format!("\"num_frames\": {}", frames));
                    let ws = ws.replace("\"width\": 1280", &format!("\"width\": {}", width));
                    let ws = ws.replace("\"height\": 720", &format!("\"height\": {}", height));
                    request.set_body(ws.as_bytes().to_vec());
                }
                _=>()
            }
            
            self.ui.label(id!((machine.id).progress)).set_text_and_redraw(cx, &format!("Started:"));
           
            request.set_metadata_id(machine.id);
            cx.http_request(live_id!(prompt), request);
                
            machine.running = MachineRunning::RunningPrompt{
                photo_name: "".to_string(),
                prompt_state: PromptState {
                    prompt: Prompt {
                        prompt: prompt.clone(),
                    },
                    seed
                },
            };
        }
    }
    
    fn send_query_to_llm(&mut self, cx: &mut Cx) {
        // alright we have a query. now what
        let url = format!("http://127.0.0.1:8080/completion");
        let mut request = HttpRequest::new(url, HttpMethod::POST);
        let mut prompt = String::new();
        
        prompt.push_str(&format!("<|begin_of_text|><|start_header_id|>system<|end_header_id|>You are an assistant that answers in very short image generator prompts of maximum 2 lines<|eot_id|>\n\n"));
        
        for (ai, msg) in &self.llm_chat{
            match ai{
               LLMMsg::Human=>prompt.push_str(&format!("<|start_header_id|>user<|end_header_id|>
                {}<|eot_id|>", msg)),
               LLMMsg::AI=>prompt.push_str(&format!("<|start_header_id|>assistant<|end_header_id|>
                {}<|eot_id|>\n", msg)),
                LLMMsg::Progress=>()
            }
        }
        
        prompt = prompt.replace("\\","").replace("\"", "\\\"").replace("\n","\\n");
        
        let body = format!("{{
            \"stream\":false,
            \"n_predict\":400,
            \"temperature\":0.7,
            \"stop\":[\"<|eot_id|>\"],
            \"repeat_last_n\":256,
            \"repeat_penalty\":1.18,
            \"top_k\":40,
            \"top_p\":0.95,
            \"min_p\":0.05,
            \"tfs_z\":1,
            \"typical_p\":1,
            \"presence_penalty\":0,
            \"frequency_penalty\":0,
            \"mirostat\":0,
            \"mirostat_tau\":5,
            \"mirostat_eta\":0.1,
            \"grammar\":\"\",
            \"n_probs\":0,
            \"min_keep\":0,
            \"image_data\":[],
            \"cache_prompt\":true,
            \"api_key\":\"\",
            \"prompt\":\"{}\"
        }}", prompt);
       
        request.set_header("Content-Type".to_string(), "application/json".to_string());
        request.set_body(body.as_bytes().to_vec());
                    
        cx.http_request(live_id!(llm), request);
    }
    
    fn cancel_machine(&mut self, cx: &mut Cx, machine_id:LiveId) {
        let machine = self.machines.iter_mut().find( | v | v.id == machine_id).unwrap();
        self.ui.label(id!((machine.id).progress)).set_text_and_redraw(cx, &format!("Stopping:"));
        
        let url = format!("http://{}/queue", machine.ip);
        let mut request = HttpRequest::new(url, HttpMethod::POST);
        let ws = "{\"clear\":true}";
        request.set_metadata_id(machine.id);
        request.set_body_string(ws);
        cx.http_request(live_id!(clear_queue), request);
                
        let url = format!("http://{}/interrupt", machine.ip);
        let mut request = HttpRequest::new(url, HttpMethod::POST);
        request.set_metadata_id(machine.id);
        cx.http_request(live_id!(interrupt), request);
        //machine.running = MachineRunning::Stopping;
    }
    
    fn fetch_image(&self, cx: &mut Cx, machine_id: LiveId, image_name: &str) {
        let machine = self.machines.iter().find( | v | v.id == machine_id).unwrap();
        let url = format!("http://{}/view?filename={}&subfolder=&type=output", machine.ip, image_name);
        let mut request = HttpRequest::new(url, HttpMethod::GET);
        request.set_metadata_id(machine.id);
        cx.http_request(live_id!(image), request);
    }
    
    fn open_web_socket(&mut self) {
        for machine in &mut self.machines {
            let url = format!("ws://{}/ws?clientId={}", machine.ip, "1234");
            let request = HttpRequest::new(url, HttpMethod::GET);
            machine.web_socket = Some(WebSocket::open(request));
        }
    }
    
    fn load_seed_from_current_image(&mut self, cx: &mut Cx) {
        if let Some(current_image) = &self.current_image {
            if let Some(image) = self.db.image_files.iter().find( | v | v.image_id == *current_image) {
                //self.last_seed = image.seed;
                //self.update_seed_display(cx);
                self.ui.text_input(id!(seed_input)).set_text_and_redraw(cx, &format!("{}",image.seed));
                
            }
        }
    }
    
    fn prompt_hash_from_current_image(&mut self) -> LiveId {
        if let Some(current_image) = &self.current_image {
            if let Some(image) = self.db.image_files.iter().find( | v | v.image_id == *current_image) {
                return image.prompt_hash
            }
        }
        LiveId(0)
    }
    /*
    fn update_seed_display(&mut self, cx: &mut Cx) {
        self.ui.text_input(id!(seed_input)).set_text_and_redraw(cx, &format!("{}", self.last_seed));
    }*/
    /*
    fn set_prompt(&mut self, cx: &mut Cx, prompt:&str) {
        self.ui.text_input(id!(prompt)).set_text_and_redraw(cx, prompt);
    }*/
    /*
    fn add_prompt(&mut self, cx: &mut Cx, prompt:&str) {
        let positive = self.ui.text_input(id!(positive)).text();
        self.ui.text_input(id!(positive)).set_text_and_redraw(cx, &format!("{} {}", positive,prompt));
    }*/
        
    fn load_inputs_from_prompt_hash(&mut self, cx: &mut Cx, prompt_hash: LiveId) {
        if let Some(prompt_file) = self.db.prompt_files.iter().find( | v | v.prompt_hash == prompt_hash) {
            self.ui.text_input(id!(prompt_input)).set_text(&prompt_file.prompt.prompt);
            self.ui.redraw(cx);
        }
    }
    
    fn load_last_sent_from_prompt_hash(&mut self, cx: &mut Cx, prompt_hash: LiveId) {
        if let Some(prompt_file) = self.db.prompt_files.iter().find( | v | v.prompt_hash == prompt_hash) {
            self.ui.text_input(id!(last_sent)).set_text(&prompt_file.prompt.prompt);
            self.ui.redraw(cx);
        }
    }
    
    fn update_textures(&mut self, cx: &mut Cx) {
        if let Some(current_image) = &self.current_image {
            let tex = self.db.image_texture(current_image);
            if tex.is_some() {
                self.ui.image_blend(id!(image_view.image)).set_texture(cx, tex.clone());
                //self.ui.image_blend(id!(big_image.image1)).set_texture(cx, tex.clone());
                self.ui.image_blend(id!(second_image.image1)).set_texture(cx, tex);
            }
        }
    }
    
    fn set_current_image(&mut self, cx: &mut Cx, image_id: ImageId) {
        // lets send the remote screens the 3 images below the current selection
        let single =  self.ui.check_box(id!(single_check_box)).selected(cx);
            
        pub fn get_data_for_index(db:&Database, current:usize, id:usize, single:bool)->Option<Vec<u8>>{
            let id = if single || db.image_files[current].prompt_hash != db.image_files[id].prompt_hash{
                current
            }
            else{
                id
            };
            if let Some(image) = db.image_files.get(id){
                if let Ok(data) = fs::read(format!("{}/{}",db.image_path, image.image_id.as_file_name())) {
                    return Some(data)
                }
            }
            None
        }
        // lets find our current image id, and we should set all to the same if the prompt hash is the same
        if let Some(current) = self.db.image_files.iter().position(|v| v.image_id == image_id){
            for (_id, ip, sender) in self.remote_screens.lock().unwrap().borrow_mut().iter(){
                log!("{:?}", ip);
                let index = if *ip == Ipv4Addr::new(10,0,0,117){1} //tv5
                else {0}; //tv3
                if let Some(data) = get_data_for_index(&self.db, current, current+index, single){
                    let _= sender.send(data);
                }
            }
        }
        /*
        */
        self.current_image = Some(image_id);
        self.update_textures(cx);
        let prompt_hash = self.prompt_hash_from_current_image();
        
        if let Some(prompt_file) = self.db.prompt_files.iter().find( | v | v.prompt_hash == prompt_hash) {
            self.ui.label(id!(second_image.prompt)).set_text(&prompt_file.prompt.prompt);
        }
    }
    
    fn select_next_image(&mut self, cx: &mut Cx) {
        self.ui.redraw(cx);
        if let Some(current_image) = &self.current_image {
            if let Some(pos) = self.filtered.flat.iter().position( | v | *v == *current_image) {
                if pos + 1 < self.filtered.flat.len() {
                    self.set_current_image(cx, self.filtered.flat[pos + 1].clone());
                    //self.last_flip = Instant::now();
                }
            }
        }
    }
    
    fn select_prev_image(&mut self, cx: &mut Cx) {
        self.ui.redraw(cx);
        if let Some(current_image) = &self.current_image {
            if let Some(pos) = self.filtered.flat.iter().position( | v | *v == *current_image) {
                if pos > 0 {
                    self.set_current_image(cx, self.filtered.flat[pos - 1].clone());
                    //self.last_flip = Instant::now();
                }
            }
        }
    }
    
    fn set_current_image_by_item_id_and_row(&mut self, cx: &mut Cx, item_id: usize, row: usize) {
        self.ui.redraw(cx);
        if let Some(ImageListItem::ImageRow {prompt_hash: _, image_count, image_files}) = self.filtered.list.get(item_id as usize) {
            self.set_current_image(cx, image_files[row.min(*image_count)].clone());
            
            //self.last_flip = Instant::now();
        }
    }
    /*
    fn save_preset(&self) -> PromptPreset {
        PromptPreset { 
            workflow: self.ui.drop_down(id!(workflow_dropdown)).selected_label(),
            width: self.ui.text_input(id!(settings_width)).text().parse::<u32>().unwrap_or(1344),
            height: self.ui.text_input(id!(settings_height)).text().parse::<u32>().unwrap_or(768),
            steps: self.ui.text_input(id!(settings_steps.input)).text().parse::<u32>().unwrap_or(20),
            cfg: self.ui.text_input(id!(settings_cfg.input)).text().parse::<f64>().unwrap_or(1.8),
            denoise: self.ui.text_input(id!(settings_denoise.input)).text().parse::<f64>().unwrap_or(1.0),
        }
    }
    
    fn load_preset(&self, preset: &PromptPreset) {
        self.ui.drop_down(id!(workflow_dropdown)).set_selected_by_label(&preset.workflow);
        self.ui.text_input(id!(settings_width)).set_text(&format!("{}", preset.width));
        self.ui.text_input(id!(settings_height)).set_text(&format!("{}", preset.height));
        self.ui.text_input(id!(settings_steps.input)).set_text(&format!("{}", preset.steps));
        self.ui.text_input(id!(settings_cfg.input)).set_text(&format!("{}", preset.cfg));
        self.ui.text_input(id!(settings_denoise.input)).set_text(&format!("{}", preset.denoise));
    }
    
    fn next_render(&mut self, cx:&mut Cx){
        if self.ui.check_box(id!(render_check_box)).selected(cx) {
            self.render(cx);
            return
        }
    }
    
    fn next_render_delay(&mut self, cx:&mut Cx){
        let delay = self.ui.text_input(id!(settings_delay.input)).text().parse::<f64>().unwrap_or(1.0);
        self.delay_timer = cx.start_timeout(delay);
    }*/
        
    pub fn start_http_server(&mut self) {
        let addr = SocketAddr::new("0.0.0.0".parse().unwrap(), 8009);
        let (tx_request, rx_request) = mpsc::channel::<HttpServerRequest> ();
        start_http_server(HttpServer {
            listen_address: addr,
            post_max_size: 1024 * 1024,
            request: tx_request
        });
        let remote_screens = self.remote_screens.clone();
        std::thread::spawn(move || {
            while let Ok(message) = rx_request.recv() {
                // only store last change, fix later
                match message {
                    HttpServerRequest::ConnectWebSocket {web_socket_id, response_sender,headers} => {
                        let ip = if let IpAddr::V4(addr) = headers.addr.ip(){
                            addr
                        }
                        else{
                            Ipv4Addr::new(0,0,0,0)
                        };
                        remote_screens.lock().unwrap().borrow_mut().push((web_socket_id,ip,response_sender));
                    },
                    HttpServerRequest::DisconnectWebSocket {web_socket_id} => {
                        remote_screens.lock().unwrap().borrow_mut().retain(|v| v.0 != web_socket_id);
                    },
                    HttpServerRequest::BinaryMessage {web_socket_id:_, response_sender: _, data:_} => {
                        //log!("GOT MESSAGE {} {}", web_socket_id, data.len());
                    }
                    HttpServerRequest::Get {headers:_, response_sender:_} => {
                    }
                    HttpServerRequest::Post {..} => { //headers, body, response}=>{
                    }
                }
            }
        });
    }
    
    pub fn start_voice_input(&mut self, cx:&mut Cx){
        if self.voice_input.is_some(){
            self.stop_voice_input(cx);
        }
        match WhisperProcess::new(){
            Ok(wp)=>{
                self.voice_input = Some(wp);
            }
            Err(e)=>{
                log!("Cannot start whisper {e}");
            }
        }
    }
    
    pub fn stop_voice_input(&mut self, _cx:&mut Cx){
        if let Some(voice_input) = self.voice_input.take(){
            voice_input.stop();
        }
    }
}

impl MatchEvent for App {
    fn handle_midi_ports(&mut self, _cx: &mut Cx, _ports: &MidiPortsEvent) {
        //cx.use_midi_inputs(&ports.all_inputs());
    }
    
    fn handle_startup(&mut self, cx:&mut Cx){
        self.open_web_socket();
        let _ = self.db.load_database();
        self.filtered.filter_db(&self.db, "", false);
        self.delay_timer = cx.start_interval(0.016);
        //self.update_seed_display(cx);
        self.start_http_server();
    }
    
    fn handle_timer(&mut self, cx: &mut Cx, e:&TimerEvent){
        if self.delay_timer.is_timer(e).is_some(){
            self.check_to_render(cx);
        }
    }
    
    fn handle_signal(&mut self, cx: &mut Cx){
        if self.db.handle_decoded_images(cx) {
            self.update_textures(cx);
            self.ui.redraw(cx);
        }
        
        for m in 0..self.machines.len(){
            if let Some(socket) = self.machines[m].web_socket.as_mut(){
                match socket.try_recv(){
                    Ok(WebSocketMessage::String(s))=>{
                        if s.contains("execution_interrupted"){
                            self.machines[m].running = MachineRunning::Stopped;
                            self.ui.label(&[self.machines[m].id, live_id!(progress)]).set_text_and_redraw(cx,"Ready:");
                        } 
                        else if s.contains("crystools.monitor") {
                                                                                     
                        }
                        else if s.contains("execution_error") { // i dont care to expand the json def for this one
                            self.machines[m].running = MachineRunning::Stopped;
                            self.ui.label(&[self.machines[m].id, live_id!(progress)]).set_text_and_redraw(cx,"Ready:");
                            log!("Got execution error for {} {}", self.machines[m].id, s);
                        }
                        else {
                            match ComfyUIMessage::deserialize_json(&s) {
                                Ok(data) => {
                                    if data._type == "status" {
                                        if let Some(status) = data.data.status {
                                            if status.exec_info.queue_remaining == 0 {
                                                /*if let MachineRunning::RunningPrompt{..} = &self.machines[m].running{
                                                    self.machines[m].running = MachineRunning::Stopped;
                                                    Self::update_progress(cx, &self.ui, self.machines[m].id, false, 0, 1);
                                                                                                                                                                                                                
                                                }*/
                                            }
                                        }
                                    }
                                    else if data._type == "executed" {
                                        if let Some(output) = &data.data.output {
                                            if let Some(image) = output.images.first() {
                                                if let MachineRunning::RunningPrompt{prompt_state, photo_name} = &self.machines[m].running{
                                                    self.machines[m].fetching = Some((photo_name.clone(), prompt_state.clone()));
                                                    self.machines[m].running = MachineRunning::Stopped;
                                                    //self.ui.text_input(id!(settings_total_steps.input)).set_text(&format!("{}", running.steps_counter));
                                                    //Self::update_progress(cx, &self.ui, self.machines[m].id, false, 0, 1);
                                                    self.ui.label(&[self.machines[m].id, live_id!(progress)]).set_text_and_redraw(cx,"Ready:");
                                                    
                                                    self.fetch_image(cx, self.machines[m].id, &image.filename);
                                                }
                                            }
                                        }
                                    }
                                    else if data._type == "progress"{
                                        // lets check which machine it is
                                        self.ui.label(&[self.machines[m].id, live_id!(progress)]).set_text_and_redraw(cx, &format!("Step: {}/{}", data.data.value.unwrap_or(0), data.data.max.unwrap_or(0)));
                                    }
                                }
                                Err(err) => {
                                    log!("Error parsing JSON {:?} {:?}", err, s);
                                }
                            }
                        }
                        self.handle_signal(cx);
                        
                    }
                    _=>()
                }
            }
        }
        
        while let Ok((id, mut vfb)) = self.video_recv.try_recv() {
            let (img_width, img_height) = self.video_input[0].get_format(cx).vec_width_height().unwrap();
            if img_width != vfb.format.width / 2 || img_height != vfb.format.height {
                self.video_input[id] = Texture::new_with_format(cx, TextureFormat::VecBGRAu8_32{
                    updated: TextureUpdated::Full,
                    data: Some(vec![]),
                    width: vfb.format.width/2,
                    height: vfb.format.height
                });
            }
            if let Some(buf) = vfb.as_vec_u32() {
                let mut buf2 = self.video_input[id].take_vec_u32(cx);
                std::mem::swap(&mut buf2, buf);
                self.video_input[id].put_back_vec_u32(cx, buf2, None);
            }
            let image_size = [vfb.format.width as f32, vfb.format.height as f32];
            let v = self.ui.image(id!(video_input0));
            v.set_texture(cx, Some(self.video_input[id].clone()));
            v.set_uniform(cx, id!(image_size), &image_size);
            v.set_uniform(cx, id!(is_rgb), &[0.0]);
            v.redraw(cx);
        }
    }
            
    fn handle_network_responses(&mut self, cx: &mut Cx, event:&NetworkResponsesEvent) {
        let image_list = self.ui.portal_list(id!(image_list));
        for event in event{
            match &event.response {
                NetworkResponse::HttpResponse(res) => {
                    // alright we got an image back
                    match event.request_id {
                        live_id!(llm)=>if let Some(res) = res.get_string_body() {
                            // lets parse it as json
                            if let Ok(val) = JsonValue::deserialize_json(&res){
                                if let Some(val) = val.key("content"){
                                    if let Some(val) = val.string(){
                                        if let Some((LLMMsg::Progress,_)) = self.llm_chat.last(){
                                            self.llm_chat.pop();
                                        }
                                        let val = val.strip_prefix("assistant").unwrap_or(val);
                                        let val = val.to_string().replace("\"","");
                                        let val = val.trim();
                                        self.ui.text_input(id!(prompt_input)).set_text(&val);
                                        self.llm_chat.push((LLMMsg::AI,val.into()));
                                        self.ui.widget(id!(llm_chat)).redraw(cx);
                                    }
                                }
                                else{
                                    log!("{}", res);
                                }
                            }
                            else{
                                log!("{}", res);
                            }
                        }
                        live_id!(prompt) => if let Some(_data) = res.get_string_body() { // lets check if the prompt executed
                        }
                        live_id!(image) => if let Some(data) = res.get_body() {
                            if let Some(machine) = self.machines.iter_mut().find( | v | {v.id == res.metadata_id}) {
                                if let Some((image_name, prompt_state)) = machine.fetching.take() {
                                                                                
                                    // lets write our image to disk properly
                                    //self.current_image = Some(
                                        //fetching.prompt_state.prompt.preset.total_steps = fetching.steps_counter as u32;
                                    let image_id = self.db.add_png_and_prompt(prompt_state, image_name, data);
                                    // scroll by one item
                                    let first_id = image_list.first_id();
                                    if first_id != 0 {
                                        image_list.set_first_id(first_id + 1);
                                    }
                                                                                    
                                    self.filtered.filter_db(&self.db, "", false);
                                    if self.db.image_texture(&image_id).is_some() {
                                        self.ui.redraw(cx);
                                    }
                                    self.set_current_image(cx, image_id);
                                    // lets select the first image 
                                }
                            }
                        }
                        live_id!(clear_queue) => {}
                        live_id!(interrupt) => {}
                        live_id!(camera) => {
                            /*
                            // move to next step
                            if let Some(machine) = self.machines.iter_mut().find( | v | {v.id == res.metadata_id}) {
                                if let MachineRunning::_UploadingImage{photo_name, prompt_state} = &machine.running{
                                                                                    
                                    let photo_name = photo_name.clone();
                                    let prompt_state = prompt_state.clone();
                                    let machine_id = machine.id;
                                    self.send_prompt_to_machine(cx, machine_id, photo_name, prompt_state)
                                }
                            }*/
                        }
                        _ => panic!()
                    }
                }
                e => {
                    log!("{} {:?}", event.request_id, e)
                }
            }
        }
    }
    
    fn handle_draw_2d(&mut self, cx:&mut Cx2d){
        
        let image_list = self.ui.portal_list(id!(image_list));
        let llm_chat = self.ui.portal_list(id!(llm_chat));
                
        while let Some(next) = self.ui.draw_unscoped(cx).step() {
           if let Some(mut llm_chat) = llm_chat.has_widget(&next).borrow_mut() {
                llm_chat.set_item_range(cx, 0, self.llm_chat.len());
                while let Some(item_id) = llm_chat.next_visible_item(cx) {
                    if item_id >= self.llm_chat.len(){
                        continue
                    }
                    let (is_llm, msg) = &self.llm_chat[item_id];
                    let template = match is_llm{
                        LLMMsg::AI=>live_id!(AI),
                        LLMMsg::Human=>live_id!(Human),
                        LLMMsg::Progress=>live_id!(AI)
                    };
                    let item = llm_chat.item(cx, item_id, template);
                    item.set_text(msg);
                    item.draw_all_unscoped(cx);
                }
            }
            if let Some(mut image_list) = image_list.has_widget(&next).borrow_mut() {
                // alright now we draw the items
                image_list.set_item_range(cx, 0, self.filtered.list.len());
                                    
                while let Some(item_id) = image_list.next_visible_item(cx) {
                                            
                    if let Some(item) = self.filtered.list.get(item_id as usize) {
                        match item {
                            ImageListItem::Prompt {prompt_hash} => {
                                let group = self.db.prompt_files.iter().find( | v | v.prompt_hash == *prompt_hash).unwrap();
                                let item = image_list.item(cx, item_id, live_id!(PromptGroup));
                                item.label(id!(prompt)).set_text(&group.prompt.prompt);
                                item.draw_all(cx, &mut Scope::empty());
                            }
                            ImageListItem::ImageRow {prompt_hash: _, image_count, image_files} => {
                                let item = image_list.item(cx, item_id, id!(Empty.ImageRow1.ImageRow2)[*image_count]);
                                let rows = item.view_set(ids!(row1, row2, row3));
                                for (index, row) in rows.iter().enumerate() {
                                    if index >= *image_count {break}
                                    // alright we need to query our png cache for an image.
                                    let tex = self.db.image_texture(&image_files[index]);
                                    row.image(id!(img)).set_texture(cx, tex);
                                }
                                item.draw_all_unscoped(cx);
                            }
                        }
                    }
                }
            }
        }
    }
    
    fn handle_key_down(&mut self, cx:&mut Cx, event:&KeyEvent){
        match event{
            KeyEvent {key_code: KeyCode::ReturnKey | KeyCode::NumpadEnter, modifiers: _, ..}=>{
                return
                /*
                self.clear_todo(cx);
                if modifiers.logo || modifiers.control {
                    self.render(cx, true);
                }
                else if modifiers.shift {
                    self.render(cx, false);
                }
                else {
                    self.render(cx, false);
                }*/
            }
            KeyEvent {is_repeat: false, key_code: KeyCode::Backspace, modifiers, ..}=>{
                if modifiers.logo {
                    let prompt_hash = self.prompt_hash_from_current_image();
                    self.load_inputs_from_prompt_hash(cx, prompt_hash);
                    self.load_seed_from_current_image(cx);
                }
            }
            KeyEvent {is_repeat: false, key_code: KeyCode::KeyC,  ..} =>{
                
            }
            KeyEvent {is_repeat: false, key_code: KeyCode::KeyR, modifiers, ..} => {
                if modifiers.control || modifiers.logo {
                    self.open_web_socket();
                }
            }
            KeyEvent {is_repeat: false, key_code: KeyCode::KeyP, modifiers, ..} => {
                if modifiers.control || modifiers.logo {
                    let prompt_frame = self.ui.view(id!(second_image.prompt_frame));
                    if prompt_frame.visible() {
                        prompt_frame.set_visible_and_redraw(cx, false);
                    }
                    else {
                        //cx.set_cursor(MouseCursor::Hidden);
                        prompt_frame.set_visible_and_redraw(cx, true);
                    }
                }
            }
          
            /*        
            Event::KeyDown(KeyEvent {is_repeat: false, key_code: KeyCode::Home, modifiers, ..}) => {
                if self.ui.view(id!(big_image)).visible() || modifiers.logo {
                    self.play(cx);
                }
            }*/
            KeyEvent {key_code: KeyCode::ArrowDown, modifiers, ..} => {
                if self.ui.view(id!(big_image)).visible() || modifiers.logo {
                    self.select_next_image(cx);
                    //self.set_slide_show(cx, false);
                }
            }
            KeyEvent {key_code: KeyCode::ArrowUp, modifiers, ..} => {
                if self.ui.view(id!(big_image)).visible() || modifiers.logo {
                    self.select_prev_image(cx);
                    //self.set_slide_show(cx, false);
                }
            }
            _=>()
        }
    }
    
    fn handle_video_inputs(&mut self, _cx: &mut Cx, devices:&VideoInputsEvent){
        //log!("{:?}", devices);
        let _input = devices.find_highest_at_res(devices.find_device("Logitech BRIO"), 1600, 896, 31.0);
        //cx.use_video_input(&input);
    }
    
    fn handle_actions(&mut self, cx:&mut Cx, actions:&Actions){
        for action in actions{
            if let Some(WhisperTextInput{clear, text}) = action.downcast_ref(){
                if text != "Thank you." && !self.ui.check_box(id!(mute_check_box)).selected(cx){
                    if *clear{
                        self.ui.text_input(id!(prompt_input)).set_text("");
                    }
                    let t = self.ui.text_input(id!(prompt_input)).text();
                    self.ui.text_input(id!(prompt_input)).set_text_and_redraw(cx, &format!("{} {}",t, text));
                }
            }
        }
        
        let image_list = self.ui.portal_list(id!(image_list));
        if let Some(ke) = self.ui.view_set(ids!(image_view, big_image)).key_down(&actions) {
            match ke.key_code {
                KeyCode::ArrowDown => {
                    self.select_next_image(cx);
                    //self.set_slide_show(cx, false);
                }
                KeyCode::ArrowUp => {
                    self.select_prev_image(cx);
                    //self.set_slide_show(cx, false);
                }
                _ => ()
            }
        }

        let chat = self.ui.text_input(id!(chat));
        if let Some(val) = chat.returned(&actions){
            chat.set_text_and_redraw(cx, "");
            chat.set_cursor(0,0);
            self.llm_chat.push((LLMMsg::Human, val));
            self.llm_chat.push((LLMMsg::Progress, "... Thinking ...".into()));
            self.ui.widget(id!(llm_chat)).redraw(cx);
            self.send_query_to_llm(cx);
        }
        
        /*
        if let Some(true) = self.ui.check_box(id!(render_check_box)).changed(&actions) {
            self.render(cx);
        }*/
        
        for action in self.ui.widget_set(ids!(model)).filter_actions(&actions){
            if let Some(label) = action.widget().as_drop_down().changed_label(&actions){
                let id = LiveId::from_str(&label.to_lowercase());
                let group = action.path.from_end(2);
                self.ui.page_flip(id!((group).page_flip)).set_active_page_and_redraw(cx, id);
            }
        }
        for action in self.ui.widget_set(ids!(cancel_button)).filter_actions(&actions){
            if action.widget().as_button().clicked(&actions){
                let machine = action.path.from_end(2);
                self.cancel_machine(cx, machine);
            }
        }
        if let Some(voice) = self.ui.check_box(id!(voice_check_box)).changed(&actions) {
            if voice{
                self.start_voice_input(cx);
            }
            else{
                self.stop_voice_input(cx);
            }
        }
        
        if self.ui.button(id!(trim_button)).clicked(&actions) {
            let positive = self.ui.text_input(id!(positive)).text();
            self.llm_chat.clear();
            self.llm_chat.push((LLMMsg::AI, positive));
            self.ui.widget(id!(llm_chat)).redraw(cx);
        }
        
        if self.ui.button(id!(clear_button)).clicked(&actions) {
            self.llm_chat.clear();
            self.ui.widget(id!(llm_chat)).redraw(cx);
        }
                    
        if let Some(change) = self.ui.text_input(id!(search)).changed(&actions) {
            self.filtered.filter_db(&self.db, &change, false);
            self.ui.redraw(cx);
            image_list.set_first_id_and_scroll(0, 0.0);
        }

        for (item_id, item) in image_list.items_with_actions(&actions) {
            // check for actions inside the list item
            let rows = item.view_set(ids!(row1, row2));
            for (row_index, row) in rows.iter().enumerate() {
                if let Some(fd) = row.finger_down(&actions) {
                    self.set_current_image_by_item_id_and_row(cx, item_id, row_index);
                    //self.set_slide_show(cx, false);
                    if fd.tap_count == 2 {
                        if let ImageListItem::ImageRow {prompt_hash, ..} = self.filtered.list[item_id as usize] {
                            self.load_seed_from_current_image(cx);
                            self.load_inputs_from_prompt_hash(cx, prompt_hash);
                        }
                    }
                    if fd.tap_count == 1 {
                        if let ImageListItem::ImageRow {prompt_hash, ..} = self.filtered.list[item_id as usize] {
                            self.load_last_sent_from_prompt_hash(cx, prompt_hash);
                        }
                    }
                }
            }
            if let Some(fd) = item.as_view().finger_down(&actions) {
                
                if fd.tap_count == 2 {
                    if let ImageListItem::Prompt {prompt_hash} = self.filtered.list[item_id as usize] {
                        self.load_inputs_from_prompt_hash(cx, prompt_hash);
                    }
                }
            }
        }
    }
        
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if self.match_event_with_draw_2d(cx, event).is_ok(){
            return
        }
        
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
