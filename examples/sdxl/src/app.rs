use crate::{makepad_live_id::*};
use makepad_micro_serde::*;
use makepad_widgets::*;
use makepad_platform::thread::*;
use makepad_widgets::image_cache::{ImageBuffer};
use std::fs;
use std::time::Instant;
use std::collections::HashMap;

live_design!{
    import makepad_widgets::button::Button;
    import makepad_widgets::desktop_window::DesktopWindow;
    import makepad_widgets::label::Label;
    import makepad_widgets::image::Image;
    import makepad_widgets::text_input::TextInput;
    import makepad_widgets::image::Image;
    import makepad_widgets::list_view::ListView;
    import makepad_widgets::slide_panel::SlidePanel;
    import makepad_widgets::frame::*;
    import makepad_widgets::theme::*;
    import makepad_draw::shader::std::*;
    import makepad_widgets::dock::*;
    
    COLOR_PANEL_BG = #x00000033
    TEXT_BOLD = {
        font_size: 12,
        font: {path: dep("crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf")}
    }

    ImageTile = <Frame> {
        walk: {width: Fill, height: Fit},
        cursor: Hand
        img = <Image> {
            walk: {width: Fill, height: Fill}
            fit: Horizontal,
            draw_bg: {
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                    sdf.box(1, 1, self.rect_size.x - 2, self.rect_size.y - 2, 4.0)
                    let color = self.get_color();
                    sdf.fill(color);
                    return sdf.result
                }
            }
        }
    }
    
    App = {{App}} {
        last_seed: 1000;
        batch_size: 6;
        ui: <DesktopWindow> {
            window: {inner_size: vec2(2000, 1024)},
            caption_bar = {visible: true, caption_label = {label = {label: "SDXL Explorer"}}},
            
            <Frame> {
                layout: {
                    flow: Overlay,
                },
                walk: {
                    width: Fill,
                    height: Fill
                },
                
                dock = <Dock> {
                    walk: {height: Fill, width: Fill}
                    
                    root = Splitter {
                        axis: Horizontal,
                        align: FromA(300.0),
                        a: image_library,
                        b: split1
                    }
                    
                    split1 = Splitter {
                        axis: Vertical,
                        align: FromB(200.0),
                        a: image_view,
                        b: split2
                    }
                    
                    split2 = Splitter {
                        axis: Horizontal,
                        align: Weighted(0.5),
                        a: positive_panel,
                        b: split3
                    }
                    
                    split3 = Splitter {
                        axis: Horizontal,
                        align: Weighted(0.5),
                        a: negative_panel,
                        b: keyword_panel
                    }
                    
                    image_library = Tab {
                        name: ""
                        kind: ImageLibrary
                    }
                    
                    positive_panel = Tab {
                        name: ""
                        kind: PositivePanel
                    }

                    negative_panel = Tab {
                        name: ""
                        kind: NegativePanel
                    }

                    keyword_panel = Tab {
                        name: ""
                        kind: KeywordPanel
                    }

                    
                    image_view = Tab {
                        name: ""
                        kind: ImageView
                    }
                    
                    ImageView = <Rect> {
                        draw_bg: {color: (COLOR_PANEL_BG)}
                        walk: {height: Fill, width: Fill}
                        layout: {flow: Down, align: {x: 0.5, y: 0.5}}
                        cursor: Hand,
                        image = <Image> {
                            fit: Smallest,
                            walk: {width: Fill, height: Fill}
                        }
                    }
                    
                    PositivePanel = <Rect> {
                        walk: {height: Fill, width: Fill}
                        layout: {flow: Right, padding: 0}
                        draw_bg: {color: (COLOR_PANEL_BG)}
                
                        positive = <TextInput> {
                            walk: {width: Fill, height: Fill},
                            text: "Purple tomatoes Purple tomatoes Purple tomatoes Purple tomatoes Purple tomatoes Purple tomatoes Purple tomatoes Purple tomatoes Purple tomatoes Purple tomatoes Purple tomatoes Purple tomatoes Purple tomatoes Purple tomatoes Purple tomatoes Purple tomatoes "
                            draw_label:{text_style:{font_size:12}}
                            draw_bg: {
                                color: #1113
                            }
                        }
                    }
                    NegativePanel = <Rect>{
                        draw_bg: {color: (COLOR_PANEL_BG)}

                        negative = <TextInput> {
                            walk: {width: Fill, height: Fill},
                            draw_label:{text_style:{font_size:12}}
                            text: "text, watermark, cartoon"
                            draw_bg: {
                                color: #1113
                            }
                        }
                    }
                    
                    KeywordPanel = <Frame>{
                        
                    }
                    
                    ImageLibrary = <Rect> {
                        draw_bg: {color: (COLOR_PANEL_BG)}
                        walk: {height: Fill, width: Fill}
                        layout: { flow: Down},
                        <Frame> {
                            walk: {height: Fit, width: Fill}
                            layout: {flow: Right,padding:{left:10, right:10, top:10}},
                            search = <TextInput> {
                                walk: {height: Fit, width: Fill}
                                empty_message: "Search"
                                draw_bg: {
                                    color: #x00000066
                                }
                            }
                        }
                        image_list = <ListView> {
                            walk: {height: Fill, width: Fill}
                            layout: {flow: Down, padding: 10}
                            PromptGroup = <Frame> {
                                walk: {height: Fit, width: Fill, margin: {bottom: 10}}
                                layout: {flow: Right, spacing: 10}
                                prompt = <Label> {
                                    walk: { width: Fill, margin: {top: 20}}
                                    draw_label:{
                                        text_style: <TEXT_BOLD>{},
                                        fn get_color(self) -> vec4 {
                                            return #CCCCCC
                                        }
                                        wrap: Word,
                                    }
                                    label: "Lorem Ipsum dolor sit amet dolorem simus sitis sint agricola servus est."
                                }
                            }
                            Empty = <Frame> {}
                            ImageRow1 = <Frame> {
                                walk: {height: Fit, width: Fill, margin: {bottom: 10}}
                                layout: {spacing: 20, flow: Right},
                                row1 = <ImageTile> {}
                            }
                            ImageRow2 = <Frame> {
                                walk: {height: Fit, width: Fill, margin: {bottom: 10}}
                                layout: {spacing: 20, flow: Right},
                                row1 = <ImageTile> {}
                                row2 = <ImageTile> {}
                            }
                            ImageRow3 = <Frame> {
                                walk: {height: Fit, width: Fill, margin: {bottom: 10}}
                                layout: {spacing: 20, flow: Right},
                                row1 = <ImageTile> {}
                                row2 = <ImageTile> {}
                                row3 = <ImageTile> {}
                            }
                        }
                    }
                }
                
                big_image = <Rect> {
                    visible: false,
                    draw_bg:{draw_depth:10.0}
                    draw_bg: {color: #0}
                    walk: {height: All, width: All, abs_pos:vec2(0.0,0.0)}
                    layout: {flow: Down, align: {x: 0.5, y: 0.5}}
                    cursor: Hand,
                    image = <Image> {
                        draw_bg:{draw_depth:11.0}
                        fit: Smallest,
                        walk: {width: Fill, height: Fill}
                    }
                }
            }
        }
    }
}

app_main!(App);

struct Machine {
    ip: String,
    id: LiveId,
    running: Option<RunningPrompt>,
    fetching: Option<RunningPrompt>
}

struct RunningPrompt {
    _started: Instant,
    prompt_state: PromptState,
}

impl Machine {
    fn new(ip: &str, id: LiveId) -> Self {Self {
        ip: ip.to_string(),
        id,
        running: None,
        fetching: None
    }}
}

#[derive(Clone)]
struct PromptState {
    prompt: Prompt,
    workflow: String,
    seed: u64
}

#[derive(Clone, DeJson, SerJson)]
struct Prompt {
    positive: String,
    negative: String,
}

impl Prompt {
    fn hash(&self) -> LiveId {
        LiveId::from_str(&self.positive).str_append(&self.negative)
    }
}

#[derive(Live)]
pub struct App {
    #[live] ui: WidgetRef,
    #[rust(vec![
        Machine::new("192.168.1.62:8188", id_lut!(m1)),
        Machine::new("192.168.1.204:8188", id_lut!(m2)),
        Machine::new("192.168.1.154:8188", id_lut!(m3)),
        Machine::new("192.168.1.144:8188", id_lut!(m4)),
        Machine::new("192.168.1.59:8188", id_lut!(m7)),
        Machine::new("192.168.1.180:8188", id_lut!(m8))
    ])] machines: Vec<Machine>,
    #[rust] queue: Vec<PromptState>,
    
    #[rust(Database::new(cx))] db: Database,
    
    #[rust] filtered: FilteredDb,
    #[rust] num_images: u64,
    #[live] batch_size: i64,
    #[live] last_seed: i64,
    
    #[rust] current_image: Option<ImageId>
}

const LIBRARY_ROWS: usize = 1;

enum ImageListItem {
    PromptGroup {group_id: usize},
    ImageRow {
        group_id: usize,
        image_count: usize,
        image_ids: [usize; LIBRARY_ROWS]
    }
}

#[allow(dead_code)]
struct ImageFile {
    starred: bool,
    file_name: String,
    workflow: String,
    seed: u64
}

struct PromptGroup {
    starred: bool,
    hash: LiveId,
    prompt: Option<Prompt>,
    images: Vec<ImageFile>
}

#[allow(dead_code)]
struct TextureItem {
    last_seen: Instant,
    texture: Texture
}

enum DecoderToUI {
    Error(String),
    Done(String, ImageBuffer)
}

#[derive(Clone, Copy, PartialEq)]
struct ImageId {
    group_id: usize,
    image_id: usize,
}

struct Database {
    image_path: String,
    groups: Vec<PromptGroup>,
    textures: HashMap<String, TextureItem>,
    in_flight: Vec<String>,
    thread_pool: ThreadPool<()>,
    to_ui: ToUIReceiver<DecoderToUI>,
}

#[derive(Default)]
struct FilteredDb {
    list: Vec<ImageListItem>,
    flat: Vec<ImageId>,
}

impl FilteredDb {
    
    fn filter_db(&mut self, db: &Database, search: &str, starred: bool) {
        self.list.clear();
        self.flat.clear();
        for (group_id, group) in db.groups.iter().enumerate() {
            if search.len() == 0
                || group.prompt.as_ref().unwrap().positive.contains(search)
                || group.prompt.as_ref().unwrap().negative.contains(search) {
                self.list.push(ImageListItem::PromptGroup {group_id});
                // lets collect images in pairs of 3
                for (store_index, image) in group.images.iter().enumerate() {
                    if starred && !image.starred {
                        continue
                    }
                    self.flat.push(ImageId {
                        group_id,
                        image_id: store_index
                    });
                    if let Some(ImageListItem::ImageRow {group_id: _, image_count, image_ids}) = self.list.last_mut() {
                        if *image_count<LIBRARY_ROWS {
                            image_ids[*image_count] = store_index;
                            *image_count += 1;
                            continue;
                        }
                    }
                    self.list.push(ImageListItem::ImageRow {group_id, image_count: 1, image_ids: [store_index]});
                    
                }
            }
        }
        
    }
    
}


impl Database {
    fn new(cx: &mut Cx) -> Self {
        let use_cores = cx.cpu_cores().max(3) - 2;
        Self {
            textures: HashMap::new(),
            in_flight: Vec::new(),
            thread_pool: ThreadPool::new(cx, use_cores),
            image_path: "./sdxl_images".to_string(),
            groups: Vec::new(),
            to_ui: ToUIReceiver::default(),
        }
    }
    
    fn handle_decoded_images(&mut self, cx: &mut Cx) -> bool {
        let mut updates = false;
        while let Ok(msg) = self.to_ui.receiver.try_recv() {
            match msg {
                DecoderToUI::Done(file_name, image_buffer) => {
                    let index = self.in_flight.iter().position( | v | *v == file_name).unwrap();
                    self.in_flight.remove(index);
                    self.textures.insert(file_name, TextureItem {
                        last_seen: Instant::now(),
                        texture: image_buffer.into_new_texture(cx)
                    });
                    updates = true;
                }
                DecoderToUI::Error(file_name) => {
                    let index = self.in_flight.iter().position( | v | *v == file_name).unwrap();
                    self.in_flight.remove(index);
                }
            }
        }
        updates
    }
    
    fn get_image_texture(&mut self, image: ImageId) -> Option<Texture> {
        let group = &self.groups[image.group_id];
        let image = &group.images[image.image_id];
        if self.in_flight.contains(&image.file_name) {
            return None
        }
        if let Some(texture) = self.textures.get(&image.file_name) {
            return Some(texture.texture.clone());
        }
        // request decode
        let file_name = image.file_name.clone();
        let image_path = self.image_path.clone();
        let to_ui = self.to_ui.sender();
        self.in_flight.push(file_name.clone());
        self.thread_pool.execute(move | _ | {
            
            if let Ok(data) = fs::read(format!("{}/{}", image_path, file_name)) {
                if let Ok(image_buffer) = ImageBuffer::from_png(&data) {
                    let _ = to_ui.send(DecoderToUI::Done(file_name, image_buffer));
                    return
                }
            }
            let _ = to_ui.send(DecoderToUI::Error(file_name));
        });
        None
    }
    
    fn load_database(&mut self) -> std::io::Result<()> {
        // alright lets read the entire directory list
        let entries = fs::read_dir(&self.image_path) ?;
        for entry in entries {
            let entry = entry ?;
            let file_name = entry.file_name().to_str().unwrap().to_string();
            if let Some(name) = file_name.strip_suffix(".json") {
                let mut starred = false;
                let name = if let Some(name) = name.strip_prefix("star_") {starred = true; name}else {name};
                
                if let Ok(hash) = name.parse::<u64>() {
                    let hash = LiveId(hash);
                    if let Ok(v) = fs::read_to_string(format!("{}/{}", self.image_path, file_name)) {
                        if let Ok(prompt) = Prompt::deserialize_json(&v) {
                            if prompt.hash() == hash {
                                //ok lets create a group
                                if let Some(group) = self.groups.iter_mut().find( | v | v.hash == hash) {
                                    group.prompt = Some(prompt);
                                    group.starred = starred;
                                }
                                else {
                                    self.groups.push(PromptGroup {
                                        hash,
                                        starred,
                                        prompt: Some(prompt),
                                        images: Vec::new()
                                    });
                                }
                            }
                            else {
                                log!("prompt hash invalid for json {}", file_name);
                            }
                        }
                    }
                }
            }
            if let Some(name) = file_name.strip_suffix(".png") {
                let mut starred = false;
                let name = if let Some(name) = name.strip_prefix("star_") {starred = true; name}else {name};
                
                let parts = name.split("_").collect::<Vec<&str >> ();
                if parts.len() == 3 {
                    if let Ok(hash) = parts[0].parse::<u64>() {
                        let hash = LiveId(hash);
                        if let Ok(seed) = parts[2].parse::<u64>() {
                            let workflow = parts[1].to_string();
                            let image_item = ImageFile {
                                starred,
                                seed,
                                workflow,
                                file_name
                            };
                            if let Some(group) = self.groups.iter_mut().find( | v | v.hash == hash) {
                                group.images.push(image_item)
                            }
                            else {
                                self.groups.push(PromptGroup {
                                    hash,
                                    starred: false,
                                    prompt: None,
                                    images: vec![image_item]
                                });
                            }
                        }
                    }
                }
            }
        }
        self.groups.retain( | v | v.prompt.is_some());
        Ok(())
    }
    
    fn add_png_and_prompt(&mut self, state: PromptState, image: &[u8]) {
        let hash = state.prompt.hash();
        let prompt_file = format!("{}/{:#016}.json", self.image_path, hash.0);
        let _ = fs::write(&prompt_file, state.prompt.serialize_json());
        let image_file = format!("{}/{:#016}_{}_{}.png", self.image_path, hash.0, state.workflow, state.seed);
        let _ = fs::write(&image_file, &image);
    }
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
    }
    
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.open_web_socket(cx);
        let _ = self.db.load_database();
        self.filtered.filter_db(&self.db, "", false);
    }
}

impl App {
    fn send_prompt(&mut self, cx: &mut Cx, prompt_state: PromptState) {
        // lets find a machine with the minimum queue size and
        for machine in &mut self.machines {
            if machine.running.is_some() {
                continue
            }
            let url = format!("http://{}/prompt", machine.ip);
            let mut request = HttpRequest::new(url, HttpMethod::POST);
            
            request.set_header("Content-Type".to_string(), "application/json".to_string());
            
            let ws = fs::read_to_string(format!("examples/sdxl/workspace_{}.json", prompt_state.workflow)).unwrap();
            let ws = ws.replace("CLIENT_ID", "1234");
            let ws = ws.replace("TEXT_INPUT", &prompt_state.prompt.positive.replace("\n","").replace("\"",""));
            let ws = ws.replace("KEYWORD_INPUT", &prompt_state.prompt.positive.replace("\n","").replace("\"",""));
            let ws = ws.replace("NEGATIVE_INPUT", &prompt_state.prompt.negative.replace("\n","").replace("\"",""));
            let ws = ws.replace("11223344", &format!("{}", prompt_state.seed));
            // lets store that we queued this image
            request.set_request_id(live_id!(prompt));
            request.set_body(ws.as_bytes().to_vec());
            cx.http_request(machine.id, request);
            machine.running = Some(RunningPrompt {
                prompt_state: prompt_state.clone(),
                _started: Instant::now(),
            });
            return
        }
        self.queue.push(prompt_state);
    }
    
    fn fetch_image(&self, cx: &mut Cx, machine_id: LiveId, image_name: &str) {
        let machine = self.machines.iter().find( | v | v.id == machine_id).unwrap();
        let url = format!("http://{}/view?filename={}&subfolder=&type=output", machine.ip, image_name);
        let mut request = HttpRequest::new(url, HttpMethod::GET);
        request.set_request_id(live_id!(image));
        cx.http_request(machine.id, request);
    }
    
    fn open_web_socket(&self, cx: &mut Cx) {
        for machine in &self.machines {
            let url = format!("ws://{}/ws?clientId={}", machine.ip, "1234");
            let request = HttpRequest::new(url, HttpMethod::GET);
            cx.web_socket_open(machine.id, request);
        }
    }
    
    fn set_progress(&mut self, cx: &mut Cx, value: &str) {
        let label = self.ui.get_label(id!(message_label));
        label.set_label(value);
        label.redraw(cx);
    }
    
    fn set_current_image_by_item_id_and_row(&mut self, cx: &mut Cx, item_id: u64, row: usize) {
        self.ui.redraw(cx);
        if let Some(ImageListItem::ImageRow {group_id, image_count, image_ids}) = self.filtered.list.get(item_id as usize) {
            self.current_image = Some(ImageId {
                group_id: *group_id,
                image_id: image_ids[row.min(*image_count)]
            })
        }
    }
    
    
    fn select_next_image(&mut self, cx: &mut Cx) {
        self.ui.redraw(cx);
        if let Some(current_image) = self.current_image {
            if let Some(pos) = self.filtered.flat.iter().position( | v | *v == current_image) {
                if pos + 1 < self.filtered.flat.len() {
                    self.current_image = Some(self.filtered.flat[pos + 1]);
                }
            }
        }
    }
    
    
    fn select_prev_image(&mut self, cx: &mut Cx) {
        self.ui.redraw(cx);
        if let Some(current_image) = self.current_image {
            if let Some(pos) = self.filtered.flat.iter().position( | v | *v == current_image) {
                if pos > 0 {
                    self.current_image = Some(self.filtered.flat[pos - 1]);
                }
            }
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if self.db.handle_decoded_images(cx) {
            self.ui.redraw(cx);
        }
        
        let image_list = self.ui.get_list_view_set(ids!(image_list));
        if let Event::Draw(event) = event {
            let cx = &mut Cx2d::new(cx, event);
            if let Some(current_image) = self.current_image {
                let tex = self.db.get_image_texture(current_image);
                self.ui.get_image(id!(image_view.image)).set_texture(tex.clone());
                self.ui.get_image(id!(big_image.image)).set_texture(tex);
            }
            
            while let Some(next) = self.ui.draw_widget(cx).hook_widget() {
                
                if let Some(mut image_list) = image_list.has_widget(&next).borrow_mut() {
                    // alright now we draw the items
                    image_list.set_item_range(0, self.filtered.list.len() as u64, 1);
                    while let Some(item_id) = image_list.next_visible_item(cx) {
                        if let Some(item) = self.filtered.list.get(item_id as usize) {
                            match item {
                                ImageListItem::PromptGroup {group_id} => {
                                    let group = &self.db.groups[*group_id];
                                    let item = image_list.get_item(cx, item_id, live_id!(PromptGroup)).unwrap();
                                    item.get_text_input(id!(prompt)).set_text(&group.prompt.as_ref().unwrap().positive);
                                    item.draw_widget_all(cx);
                                }
                                ImageListItem::ImageRow {group_id, image_count, image_ids} => {
                                    let item = image_list.get_item(cx, item_id, id!(Empty.ImageRow1.ImageRow2)[*image_count]).unwrap();
                                    let rows = item.get_frame_set(ids!(row1, row2, row3));
                                    for (index, row) in rows.iter().enumerate() {
                                        if index >= *image_count {break}
                                        // alright we need to query our png cache for an image.
                                        let image_id = ImageId {group_id: *group_id, image_id: image_ids[index]};
                                        let tex = self.db.get_image_texture(image_id);
                                        row.get_image(id!(img)).set_texture(tex);
                                    }
                                    item.draw_widget_all(cx);
                                }
                            }
                        }
                    }
                    
                }
                
            }
            return
        }
        
        for event in event.network_responses() {
            match &event.response {
                NetworkResponse::WebSocketString(s) => {
                    if s.contains("execution_error") { // i dont care to expand the json def for this one
                        log!("Got execution error for {} {}", event.id, s);
                    }
                    else {
                        match ComfyUIMessage::deserialize_json(&s) {
                            Ok(data) => {
                                if data._type == "status" {
                                    if let Some(status) = data.data.status {
                                        if status.exec_info.queue_remaining == 0 {
                                            if let Some(machine) = self.machines.iter_mut().find( | v | {v.id == event.id}) {
                                                machine.running = None;
                                            }
                                            if let Some(prompt) = self.queue.pop() {
                                                self.send_prompt(cx, prompt);
                                            }
                                        }
                                    }
                                }
                                else if data._type == "executed" {
                                    if let Some(output) = &data.data.output {
                                        if let Some(image) = output.images.first() {
                                            if let Some(machine) = self.machines.iter_mut().find( | v | {v.id == event.id}) {
                                                if let Some(running) = machine.running.take(){
                                                    machine.fetching = Some(running);
                                                    self.fetch_image(cx, event.id, &image.filename);
                                                }
                                            }
                                        }
                                    }
                                }
                                else if data._type == "progress" {
                                    // draw the progress bar / progress somewhere
                                    self.set_progress(cx, &format!("Step {}/{}", data.data.value.unwrap_or(0), data.data.max.unwrap_or(0)))
                                }
                            }
                            Err(err) => {
                                log!("Error parsing JSON {:?} {:?}", err, s);
                            }
                        }
                    }
                }
                NetworkResponse::WebSocketBinary(bin) => {
                    log!("Got Binary {}", bin.len());
                }
                NetworkResponse::HttpResponse(res) => {
                    // prompt request
                    if let Some(machine) = self.machines.iter_mut().find( | v | {v.id == event.id}) {
                        // alright we got an image back
                        match res.request_id {
                            live_id!(prompt) => if let Some(_data) = res.get_string_body() { // lets check if the prompt executed
                            }
                            live_id!(image) => if let Some(data) = res.get_body() {
                                if let Some(fetching) = machine.fetching.take() {
                                    
                                    // lets write our image to disk properly
                                    self.db.add_png_and_prompt(fetching.prompt_state, data);
                                    
                                    // alright we got a png. lets decode it and stuff it in our image viewer
                                    let big_list = self.ui.get_list_view(id!(big_list));
                                    let image_id = self.num_images;
                                    self.num_images += 1;
                                    let item = big_list.get_item(cx, image_id, live_id!(Image)).unwrap().as_image();
                                    item.load_png_from_data(cx, data);
                                    
                                    self.ui.redraw(cx);
                                }
                                
                            }
                            _ => panic!()
                        }
                    }
                }
                e => {
                    log!("{} {:?}", event.id, e)
                }
            }
        }
        
        let actions = self.ui.handle_widget_event(cx, event);
        
        if let Event::KeyDown(KeyEvent {is_repeat: false, key_code: KeyCode::ReturnKey, ..}) = event {
            let positive = self.ui.get_text_input(id!(positive)).get_text();
            //let keyword_input = self.ui.get_text_input(id!(keyword_input)).get_text();
            let negative = self.ui.get_text_input(id!(negative)).get_text();
            for _ in 0..self.batch_size {
                self.last_seed += 1;
                self.send_prompt(cx, PromptState {
                    prompt: Prompt {
                        positive: positive.clone(),
                        negative: negative.clone(),
                    },
                    workflow: "3840".to_string(),
                    seed: self.last_seed as u64
                });
            }
            self.set_progress(cx, "Starting query");
        }
        /*
        if let Event::KeyDown(KeyEvent {is_repeat: false, key_code: KeyCode::Space, ..}) = event {
            self.ui.get_slide_panel(id!(library_panel)).toggle(cx);
            self.ui.get_slide_panel(id!(input_panel)).toggle(cx);
        }*/
        
        if let Some(change) = self.ui.get_text_input(id!(search)).changed(&actions) {
            self.filtered.filter_db(&self.db, &change, false);
            self.ui.redraw(cx);
            image_list.set_first_id(0);
        }
        /*
        if self.ui.get_button(id!(open_library)).pressed(&actions){
            self.ui.get_slide_panel(id!(library_panel)).open(cx);
            self.ui.get_slide_panel(id!(input_panel)).close(cx);
        }

        if self.ui.get_button(id!(close_library)).pressed(&actions){
            self.ui.get_slide_panel(id!(library_panel)).close(cx);
            self.ui.get_slide_panel(id!(input_panel)).open(cx);
        }
        */
        if let Some(e) = self.ui.get_frame(id!(image_view)).finger_down(&actions){
            if e.tap_count >1{
                self.ui.get_frame(id!(big_image)).set_visible(true);
                self.ui.redraw(cx);
            }
        }
        
        if let Some(e) = self.ui.get_frame(id!(big_image)).finger_down(&actions){
            if e.tap_count >1{
                self.ui.get_frame(id!(big_image)).set_visible(false);
                self.ui.redraw(cx);
            }
        }
        
        if let Some(ke) = self.ui.get_frame_set(ids!(image_view, big_image)).key_down(&actions) {
            match ke.key_code {
                KeyCode::ArrowDown => {
                    self.select_next_image(cx);
                }
                KeyCode::ArrowUp => {
                    self.select_prev_image(cx);
                }
                _ => ()
            }
        }
        
        for (item_id, item) in image_list.items_with_actions(&actions) {
            // check for actions inside the list item
            let rows = item.get_frame_set(ids!(row1, row2));
            for (index, row) in rows.iter().enumerate() {
                if row.finger_down(&actions).is_some() {
                    self.set_current_image_by_item_id_and_row(cx, item_id, index);
                }
            }
        }
    }
}

#[allow(dead_code)]
#[derive(DeJson, Debug)]
struct ComfyUINodeError {
    unknown: Option<String>
}


#[allow(dead_code)]
#[derive(DeJson, Debug)]
struct ComfyUIResponse {
    pub prompt_id: String,
    pub number: String,
    pub node_errors: ComfyUINodeError,
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

