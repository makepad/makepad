use crate::{makepad_live_id::*};
use makepad_micro_serde::*;
use makepad_widgets::*;
use std::fs;

live_design!{
    import makepad_widgets::button::Button;
    import makepad_widgets::desktop_window::DesktopWindow;
    import makepad_widgets::label::Label;
    import makepad_widgets::image::Image;
    import makepad_widgets::text_input::TextInput;
    import makepad_widgets::image::Image;
    import makepad_widgets::list_view::ListView;
    import makepad_widgets::frame::*;
    import makepad_draw::shader::std::*;
    
    App = {{App}} {
        ui: <DesktopWindow> {
            window: {inner_size: vec2(1024, 1024)},
            
            show_bg: true
            layout: {
                flow: Overlay,
            },
            walk: {
                width: Fill,
                height: Fill
            },
            draw_bg: {
                fn pixel(self) -> vec4 {
                    return mix(#3, #1, self.geom_pos.y + Math::random_2d(self.pos.xy) * 0.04);
                }
            }
            image_list = <ListView> {
                walk: {height: Fill, width: Fill}
                layout: {flow: Down}
                Image = <Image> {
                    walk: {width: 1920, height: 1080}
                }
            }            
            <Frame> {
                
                walk: {height: Fill, width: Fill}
                layout: {flow: Down}
                text_input = <TextInput> {
                    text: "Purple tomatoes"
                    walk: {width: Fill, height: Fit, margin: {top: 30, left: 20, right: 20}},
                    draw_bg: {
                        color: #1113
                    }
                }
                keyword_input = <TextInput> {
                    text: "Photographic"
                    walk: {width: Fill, height: Fit, margin: { left: 20, right: 20,}},
                    draw_bg: {
                        color: #1113
                    }
                }
                negative_input = <TextInput> {
                    text: "text, watermark, cartoon"
                    walk: {width: Fill, height: Fit, margin: {left: 20, right: 20}},
                    draw_bg: {
                        color: #1113
                    }
                } 
            }
            <Frame> {
                
                draw_bg: {color: #f00}
                layout: {
                    align: {
                        x: 0.5,
                        y: 1.0
                    }
                },
                message_label = <Label> {
                    walk: {width: Fit, height: Fit, margin: {bottom: 20}},
                    draw_label: {
                        wrap: Word
                        color: #f
                    },
                    label: "Progress",
                }
            }
        }
    }
}

app_main!(App);

struct Machine {
    ip: String,
    id: LiveId,
}
impl Machine {
    fn new(ip: &str, id: LiveId) -> Self {Self {ip: ip.to_string(), id}}
}

#[derive(Live)]
pub struct App {
    #[live] ui: WidgetRef,
    #[rust(vec![
        Machine::new("192.168.1.59:8188", live_id!(m1)),
        Machine::new("192.168.1.62:8188", live_id!(m2)),
        Machine::new("192.168.1.204:8188", live_id!(m3)),
        Machine::new("192.168.1.154:8188", live_id!(m4)),
        Machine::new("192.168.1.144:8188", live_id!(m5))
    ])] machines: Vec<Machine>,
    #[rust] num_images: u64,
    #[rust(10000u64)] last_seed: u64,
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
    }
    
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.open_web_socket(cx);
    }
}
const CLIENT_ID: &'static str = "1234";

impl App {
    fn send_prompt(&mut self, cx: &mut Cx, text_input: String, keyword_input: String, negative_input:String) {
        for machine in &self.machines {
            let url = format!("http://{}/prompt", machine.ip);
            let mut request = HttpRequest::new(url, HttpMethod::POST);
            
            request.set_header("Content-Type".to_string(), "application/json".to_string());
            
            let ws = fs::read_to_string("examples/comfyui/workspace_3000.json").unwrap();
            let ws = ws.replace("CLIENT_ID", CLIENT_ID);
            let ws = ws.replace("TEXT_INPUT", &text_input);
            let ws = ws.replace("KEYWORD_INPUT", &keyword_input);
            let ws = ws.replace("NEGATIVE_INPUT", &negative_input);
            let ws = ws.replace("11223344", &format!("{}", self.last_seed));
            self.last_seed += 1;
            request.set_body(ws.as_bytes().to_vec());
            
            cx.http_request(machine.id, request);
        }
    }
    
    fn fetch_image(&self, cx: &mut Cx, machine_id: LiveId, image_name: &str) {
        let machine = self.machines.iter().find( | v | v.id == machine_id).unwrap();
        let url = format!("http://{}/view?filename={}&subfolder=&type=output", machine.ip, image_name);
        let request = HttpRequest::new(url, HttpMethod::GET);
        cx.http_request(live_id!(fetch_image), request);
    }
    
    fn open_web_socket(&self, cx: &mut Cx) {
        for machine in &self.machines {
            let url = format!("ws://{}/ws?clientId={}", machine.ip, CLIENT_ID);
            let request = HttpRequest::new(url, HttpMethod::GET);
            cx.web_socket_open(machine.id, request);
        }
    }
    
    fn set_progress(&mut self, cx: &mut Cx, value: &str) {
        let label = self.ui.get_label(id!(message_label));
        label.set_label(value);
        label.redraw(cx);
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        let image_list = self.ui.get_list_view_set(ids!(image_list));
        if let Event::Draw(event) = event {
            let cx = &mut Cx2d::new(cx, event);
            while let Some(next) = self.ui.draw_widget(cx).hook_widget() {
                if let Some(mut list) = image_list.has_widget(&next).borrow_mut() {
                    list.set_item_range(0, self.num_images, 1);
                    while let Some(item_id) = list.next_visible_item(cx) {
                        if item_id >= self.num_images {
                            continue;
                        }
                        let item = list.get_item(cx, item_id, live_id!(Image)).unwrap();
                        item.draw_widget_all(cx);
                    }
                }
            }
            return
        }
        
        if let Event::KeyDown(KeyEvent {is_repeat: false, key_code: KeyCode::ReturnKey, ..}) = event {
            let text_input = self.ui.get_text_input(id!(text_input)).get_text();
            let keyword_input = self.ui.get_text_input(id!(text_input)).get_text();
            let negative_input = self.ui.get_text_input(id!(negative_input)).get_text();
            self.send_prompt(cx, text_input, keyword_input, negative_input);
            self.set_progress(cx, "Starting query");
        }
        
        for event in event.network_responses() {
            match &event.response {
                NetworkResponse::WebSocketString(s) => match ComfyUIMessage::deserialize_json(&s) {
                    Ok(data) => {
                        if data._type == "executed" {
                            if let Some(output) = &data.data.output {
                                if let Some(image) = output.images.first() {
                                    log!("Fetching {}", image.filename);
                                    self.fetch_image(cx, event.id, &image.filename);
                                }
                            }
                        }
                        if data._type == "progress" {
                            // draw the progress bar / progress somewhere
                            self.set_progress(cx, &format!("Step {}/{}", data.data.value.unwrap_or(0), data.data.max.unwrap_or(0)))
                        }
                    }
                    Err(err) => {
                        log!("Error parsing JSON {:?} {:?}", err, s);
                    }
                }
                NetworkResponse::WebSocketBinary(bin) => {
                    log!("Got Binary {}", bin.len());
                }
                NetworkResponse::HttpResponse(res) => if let Some(data) = res.get_body() {
                    if event.id == live_id!(fetch_image) {
                        // alright we got a png. lets decode it and stuff it in our image viewer
                        let image_list = self.ui.get_list_view(id!(image_list));
                        let image_id = self.num_images;
                        self.num_images += 1;
                        let item = image_list.get_item(cx, image_id, live_id!(Image)).unwrap().as_image();
                        item.load_png_from_data(cx, data);
                        
                        self.ui.redraw(cx);
                    }
                }
                e => {
                    log!("{} {:?}", event.id, e)
                }
            }
        }
        
        let _actions = self.ui.handle_widget_event(cx, event);
    }
}

#[allow(dead_code)]
#[derive(DeJson, Debug)]
struct ComfyUIMessage {
    pub _type: String,
    pub data: ComfyUIData
}
#[allow(dead_code)]
#[derive(DeJson, Debug)]
struct ComfyUIData {
    pub value: Option<u32>,
    pub max: Option<u32>,
    pub node: Option<String>,
    pub prompt_id: Option<String>,
    pub nodes: Option<Vec<String >>,
    pub status: Option<ComfyUIStatus>,
    pub sid: Option<String>,
    pub output: Option<ComfyUIOutput>
}

#[allow(dead_code)]
#[derive(DeJson, Debug)]
struct ComfyUIStatus {
    pub exec_info: ComfyUIExecInfo,
}

#[allow(dead_code)]
#[derive(DeJson, Debug)]
struct ComfyUIOutput {
    pub images: Vec<ComfyUIImage>,
}

#[allow(dead_code)]
#[derive(DeJson, Debug)]
struct ComfyUIImage {
    pub filename: String,
    pub subfolder: String,
    pub _type: String
}

#[allow(dead_code)]
#[derive(DeJson, Debug)]
struct ComfyUIExecInfo {
    pub queue_remaining: u32
}

