use crate::{makepad_live_id::*};
use makepad_micro_serde::*;
use makepad_widgets::*;
use makepad_platform::thread::*;
use makepad_widgets::image_cache::{ImageBuffer};
use std::fs;
use std::time::Instant;
use std::collections::HashMap;

#[derive(Clone)]
pub struct PromptState {
    pub prompt: Prompt,
    pub workflow: usize,
    pub seed: u64
}

#[derive(Clone, DeJson, SerJson)]
pub struct Prompt {
    pub positive: String,
    pub negative: String,
}

impl Prompt {
    fn hash(&self) -> LiveId {
        LiveId::from_str(&self.positive).str_append(&self.negative)
    }
}

const LIBRARY_ROWS: usize = 1;

pub enum ImageListItem {
    PromptGroup {group_id: usize},
    ImageRow {
        group_id: usize,
        image_count: usize,
        image_ids: [usize; LIBRARY_ROWS]
    }
}

#[allow(dead_code)]
pub struct ImageFile {
    pub starred: bool,
    pub file_name: String,
    pub workflow: String,
    pub seed: u64
}

pub struct PromptGroup {
    pub starred: bool,
    pub hash: LiveId,
    pub prompt: Option<Prompt>,
    pub images: Vec<ImageFile>
}

#[allow(dead_code)]
pub struct TextureItem {
    pub last_seen: Instant,
    pub texture: Texture
}

pub enum DecoderToUI {
    Error(String),
    Done(String, ImageBuffer)
}

#[derive(Clone, Copy, PartialEq)]
pub struct ImageId {
    pub group_id: usize,
    pub image_id: usize,
}

pub struct Database {
    pub image_path: String,
    pub groups: Vec<PromptGroup>,
    pub textures: HashMap<String, TextureItem>,
    pub in_flight: Vec<String>,
    pub thread_pool: ThreadPool<()>,
    pub to_ui: ToUIReceiver<DecoderToUI>,
}

#[derive(Default)]
pub struct FilteredDb {
    pub list: Vec<ImageListItem>,
    pub flat: Vec<ImageId>,
}

impl FilteredDb {
    
    pub fn filter_db(&mut self, db: &Database, search: &str, starred: bool) {
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
    pub fn new(cx: &mut Cx) -> Self {
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
    
    pub fn handle_decoded_images(&mut self, cx: &mut Cx) -> bool {
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
    
    pub fn get_image_texture(&mut self, image: ImageId) -> Option<Texture> {
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
    
    pub fn load_database(&mut self) -> std::io::Result<()> {
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
    
    pub fn add_png_and_prompt(&mut self, state: PromptState, image: &[u8]) {
        let hash = state.prompt.hash();
        let prompt_file = format!("{}/{:#016}.json", self.image_path, hash.0);
        let _ = fs::write(&prompt_file, state.prompt.serialize_json());
        let image_file = format!("{}/{:#016}_{}_{}.png", self.image_path, hash.0, state.workflow, state.seed);
        let _ = fs::write(&image_file, &image);
    }
}

