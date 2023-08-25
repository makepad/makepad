use crate::{makepad_live_id::*};
use makepad_micro_serde::*;
use makepad_widgets::*;
use makepad_platform::thread::*;
use makepad_widgets::image_cache::{ImageBuffer};
use std::fs;
use std::time::Instant;
use std::time::SystemTime;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Hash, PartialEq, Eq)]
pub struct ImageId(pub Arc<String>);

impl ImageId {
    pub fn new(val: String) -> Self {Self (Arc::new(val))}
    pub fn as_file_name(&self) -> String {
        self.0.to_string()
    }
}

#[derive(Clone)]
pub struct PromptState {
    pub prompt: Prompt,
    pub seed: u64
}

#[derive(Default, Clone, DeJson, SerJson)]
pub struct PromptPreset {
    pub workflow: String,
    pub width: u32,
    pub height: u32,
    pub steps: u32,
    pub base_cfg: f64,
    pub refiner_cfg: f64,
    pub positive_score: f64,
    pub negative_score: f64,
    pub base_start_step: u32,
    pub base_end_step: u32,
    pub refiner_start_step: u32,
    pub refiner_end_step: u32,
    pub upscale_steps: u32,
    pub upscale_start_step: u32,
    pub upscale_end_step: u32,
    pub scale: f64,
    pub total_steps: u32
}


#[derive(Default, Clone, DeJson, SerJson)]
pub struct Prompt {
    pub positive: String,
    pub negative: String,
    pub preset: PromptPreset
}

impl Prompt {
    fn hash(&self) -> LiveId {
        LiveId::from_str(&self.positive)
            .str_append(&self.negative)
            .str_append(&self.preset.workflow)
            .bytes_append(&self.preset.width.to_be_bytes())
            .bytes_append(&self.preset.height.to_be_bytes())
            .bytes_append(&self.preset.steps.to_be_bytes())
            .bytes_append(&self.preset.base_cfg.to_be_bytes())
            .bytes_append(&self.preset.refiner_cfg.to_be_bytes())
            .bytes_append(&self.preset.positive_score.to_be_bytes())
            .bytes_append(&self.preset.negative_score.to_be_bytes())
            .bytes_append(&self.preset.base_start_step.to_be_bytes())
            .bytes_append(&self.preset.base_end_step.to_be_bytes())
            .bytes_append(&self.preset.refiner_start_step.to_be_bytes())
            .bytes_append(&self.preset.refiner_end_step.to_be_bytes())
            .bytes_append(&self.preset.upscale_steps.to_be_bytes())
            .bytes_append(&self.preset.upscale_start_step.to_be_bytes())
            .bytes_append(&self.preset.upscale_end_step.to_be_bytes())
            .bytes_append(&self.preset.scale.to_be_bytes())
    }
}

const LIBRARY_ROWS: usize = 1;

pub enum ImageListItem {
    Prompt {prompt_hash: LiveId},
    ImageRow {
        prompt_hash: LiveId,
        image_count: usize,
        image_files: [ImageId; LIBRARY_ROWS]
    }
}

#[allow(dead_code)]
pub struct ImageFile {
    pub prompt_hash: LiveId,
    pub starred: bool,
    pub modified: SystemTime,
    pub image_id: ImageId,
    pub seed: u64
}

pub struct PromptFile {
    pub starred: bool,
    pub prompt_hash: LiveId,
    pub modified: SystemTime,
    pub prompt: Prompt,
}

#[allow(dead_code)]
pub struct TextureItem {
    pub last_seen: Instant,
    pub texture: Texture
}

pub enum DecoderToUI {
    Error(ImageId),
    Done(ImageId, ImageBuffer)
}

pub struct Database {
    pub image_path: String,
    
    pub prompt_files: Vec<PromptFile>,
    pub image_files: Vec<ImageFile>,
    //pub image_index: HashMap<ImageId, usize>,
    
    pub textures: HashMap<ImageId, TextureItem>,
    pub in_flight: Vec<ImageId>,
    pub thread_pool: TagThreadPool<ImageId>,
    pub to_ui: ToUIReceiver<DecoderToUI>,
}

#[derive(Default)]
pub struct FilteredDb {
    pub list: Vec<ImageListItem>,
    pub flat: Vec<ImageId>,
}

impl FilteredDb {
    
    pub fn filter_db(&mut self, db: &Database, search: &str, _starred: bool) {
        self.list.clear();
        self.flat.clear();
        
        for image in &db.image_files {
            if search.len() == 0 || 
            (if let Some(prompt_file) = db.prompt_files.iter().find( | g | g.prompt_hash == image.prompt_hash) {
                prompt_file.prompt.positive.contains(search) || prompt_file.prompt.negative.contains(search) 
            }else{false}){
                self.flat.push(image.image_id.clone());
                /*if let Some(pos) = self.list.iter().find(|v|{if let ImageListItem::Prompt{prompt_hash} = v {*prompt_hash == image.prompt_hash}else{false} }).is_none(){
                    self.list.push(ImageListItem::Prompt {
                        prompt_hash: image.prompt_hash,
                    });
                }
                if let Some(ImageListItem::ImageRow {prompt_hash: _, image_count, image_files}) = self.list.last_mut() {
                    if *image_count<LIBRARY_ROWS {
                        image_files[*image_count] = image.image_id.clone();
                        *image_count += 1;
                        continue;
                    }
                }*/
                self.list.push(ImageListItem::ImageRow {
                    prompt_hash: image.prompt_hash,
                    image_count: 1,
                    image_files: [image.image_id.clone()]
                });
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
            thread_pool: TagThreadPool::new(cx, use_cores),
            image_path: "./sdxl_images".to_string(),
            image_files: Vec::new(),
            prompt_files: Vec::new(),
            //image_index: HashMap::new(),
            to_ui: ToUIReceiver::default(),
        }
    }
    
    pub fn handle_decoded_images(&mut self, cx: &mut Cx) -> bool {
        let mut updates = false;
        while let Ok(msg) = self.to_ui.receiver.try_recv() {
            match msg {
                DecoderToUI::Done(image_id, image_buffer) => {
                    let index = self.in_flight.iter().position( | v | *v == image_id).unwrap();
                    self.in_flight.remove(index);
                    self.textures.insert(image_id, TextureItem {
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
    
    pub fn get_image_texture(&mut self, image_id: &ImageId) -> Option<Texture> {
        
        if let Some(texture) = self.textures.get_mut(&image_id) {
            texture.last_seen = Instant::now();
            return Some(texture.texture.clone());
        }
        //let image_file = &self.image_files[*self.image_index.get(&image_id).unwrap()];
        // lets see if we have too many images
        let now = Instant::now();
        while self.textures.len()>200{
            if let Some((image_id,_)) = self.textures.iter().max_by(|(_,a),(_,b)|{
                (now-a.last_seen).cmp(&(now-b.last_seen))
            }){
                let image_id = image_id.clone();
                self.textures.remove(&image_id);
            }
            else{
                break;
            }
        }
        
        // request decode
        let image_path = self.image_path.clone();
        let to_ui = self.to_ui.sender();
        self.in_flight.push(image_id.clone());
        let image_id = image_id.clone();

        self.thread_pool.execute_rev(image_id, move |image_id| {
            
            if let Ok(data) = fs::read(format!("{}/{}", image_path, image_id.as_file_name())) {
                if let Ok(image_buffer) = ImageBuffer::from_png(&data) {
                    let _ = to_ui.send(DecoderToUI::Done(image_id, image_buffer));
                    return
                }
            }
            let _ = to_ui.send(DecoderToUI::Error(image_id));
        });
        None
    }
    
    pub fn load_database(&mut self) -> std::io::Result<()> {
        // alright lets read the entire directory list
        let entries = fs::read_dir(&self.image_path) ?;
        for entry in entries {
            let entry = entry ?;
            
            let file_name = entry.file_name().to_str().unwrap().to_string();
            //log!("{:?}", entry.metadata().unwrap().created().unwrap());
            let modified = entry.metadata().unwrap().modified().unwrap();
            //let created = entry.metadata().unwrap().created().unwrap().duration_since(SystemTime::now()).unwrap();
            
            if let Some(name) = file_name.strip_suffix(".json") {
                let mut starred = false;
                let name = if let Some(name) = name.strip_prefix("star_") {starred = true; name}else {name};
                
                if let Ok(prompt_hash) = name.parse::<u64>() {
                    let prompt_hash = LiveId(prompt_hash);
                    if let Ok(v) = fs::read_to_string(format!("{}/{}", self.image_path, file_name)) {
                        if let Ok(prompt) = Prompt::deserialize_json(&v) {
                            //ok lets create a group
                            self.prompt_files.push(PromptFile {
                                prompt_hash,
                                modified,
                                starred,
                                prompt,
                            });
                        }
                    }
                }
            }
            if let Some(name) = file_name.strip_suffix(".png") {
                let mut starred = false;
                let name = if let Some(name) = name.strip_prefix("star_") {starred = true; name}else {name};
                
                let parts = name.split("_").collect::<Vec<&str >> ();
                if parts.len() == 2{
                    if let Ok(prompt_hash) = parts[0].parse::<u64>() {
                        let prompt_hash = LiveId(prompt_hash);
                        if let Ok(seed) = parts[1].parse::<u64>() {
                            self.image_files.push(ImageFile {
                                prompt_hash,
                                starred,
                                seed,
                                modified,
                                image_id: ImageId::new(file_name)
                            });
                        }
                    }
                }
            }
        }
        // lets sort everything downwards but well draw in reverse order
        // this way our cache pointers dont invalidate
        self.sort_images();
        Ok(())
    }
    
    fn sort_images(&mut self) {
        self.prompt_files.sort_by( | a, b | b.modified.cmp(&a.modified));
        self.image_files.sort_by( | a, b | b.modified.cmp(&a.modified));
    }
    
    pub fn add_png_and_prompt(&mut self, state: PromptState, image: &[u8]) -> ImageId {
        let prompt_hash = state.prompt.hash();
        let prompt_file = format!("{}/{:#016}.json", self.image_path, prompt_hash.0);
        let _ = fs::write(&prompt_file, state.prompt.serialize_json());
        let file_name = format!("{:#016}_{}.png", prompt_hash.0, state.seed);
        let full_path = format!("{}/{}", self.image_path, file_name);
        let _ = fs::write(&full_path, &image);
        // ok lets see if we need to add a group, or an image
        let image_id = ImageId::new(file_name);
        self.image_files.push(ImageFile {
            prompt_hash,
            starred: false,
            seed: state.seed,
            modified: SystemTime::now(),
            image_id: image_id.clone()
        });
        if self.prompt_files.iter().find( | v | v.prompt_hash == prompt_hash).is_none() {
            self.prompt_files.push(PromptFile {
                starred: false,
                prompt_hash,
                modified: SystemTime::now(),
                prompt: state.prompt,
            });
        }
        self.sort_images();
        image_id
    }
}

