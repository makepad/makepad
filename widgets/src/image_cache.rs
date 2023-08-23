use crate::{makepad_draw::*};
use std::collections::HashMap;
use makepad_zune_jpeg::JpegDecoder;
use makepad_zune_png::PngDecoder;

#[derive(Default, Clone)] 
pub struct ImageBuffer {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u32>,
}

impl ImageBuffer {
    pub fn new(in_data: &[u8], width: usize,height: usize) -> Result<ImageBuffer, String> {
        let mut out = Vec::new();
        let pixels = width * height;
        out.resize(pixels, 0u32);
        // input pixel packing
        if in_data.len() /  pixels== 3{
            for i in 0..pixels{
                let r = in_data[i*3];
                let g = in_data[i*3+1];
                let b = in_data[i*3+2];
                out[i] = 0xff000000 | ((r as u32)<<16) | ((g as u32)<<8) | ((b as u32)<<0);
            }
        }
        else if in_data.len() / pixels == 4{
            for i in 0..pixels{
                let r = in_data[i*4];
                let g = in_data[i*4+1];
                let b = in_data[i*4+2];
                let a = in_data[i*4+3];
                out[i] = ((a as u32)<<24) | ((r as u32)<<16) | ((g as u32)<<8) | ((b as u32)<<0);
            }
        }
        else{
            return Err("ImageBuffer::new Image buffer pixel alignment not 3 or 4".to_string())
        }
        Ok(ImageBuffer {
            width,
            height,
            data: out
        })
    }
    
    pub fn into_new_texture(self, cx:&mut Cx)->Texture{
        let texture = Texture::new(cx);
        self.into_texture(cx, &texture);
        texture
    }
    
    pub fn into_texture(mut self, cx:&mut Cx, texture:&Texture){
        texture.set_desc(
            cx,
            TextureDesc {
                format: TextureFormat::ImageBGRA,
                width: Some(self.width),
                height: Some(self.height),
            },
        );
        texture.swap_image_u32(cx, &mut self.data);
    }
    
    
    pub fn from_png(
        data: &[u8]
    ) -> Result<Self, String> {
        let mut decoder = PngDecoder::new(data);
        match decoder.decode() {
            Ok(image) => {
                if let Some(data) = image.u8(){
                    let (width,height) = decoder.get_dimensions().unwrap();
                    ImageBuffer::new(&data, width as usize, height as usize)
                }
                else{
                    Err("Error decoding PNG: image data empty".to_string())
                }
            }
            Err(err) => {
                Err(format!("Error decoding PNG: {:?}", err))
            }
        }
    }

    pub fn from_jpg(
        data: &[u8]
    ) -> Result<Self, String> {
        let mut decoder = JpegDecoder::new(&*data);
        // decode the file
        match decoder.decode() {
            Ok(data) => {
                let info = decoder.info().unwrap();
                ImageBuffer::new(&data, info.width as usize, info.height as usize)
            },
            Err(err) => {
                Err(format!("Error decoding JPG: {:?}", err))
            }
        }
    }
}

pub struct ImageCache {
    map: HashMap<String, Texture>,
}

impl ImageCache {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }
}

pub trait ImageCacheImpl {
    fn get_texture(&self) -> &Option<Texture>;
    fn set_texture(&mut self, texture: Option<Texture>);

    fn lazy_create_image_cache(&mut self,cx: &mut Cx) {
        if !cx.has_global::<ImageCache>() {
            cx.set_global(ImageCache::new());
        }
    }


    fn load_png_from_data(&mut self, cx:&mut Cx, data:&[u8]){
        match ImageBuffer::from_png(&*data){
            Ok(data)=>{
                if let Some(texture) = self.get_texture(){
                    data.into_texture(cx, texture);
                }
                else{
                    self.set_texture(Some(data.into_new_texture(cx)));
                }
            }
            Err(err)=>{
                error!("load_png_from_data: Cannot load png image from data {}", err);
            }
        }
    }
    
    fn load_jpg_from_data(&mut self, cx:&mut Cx, data:&[u8]){
        match ImageBuffer::from_jpg(&*data){
            Ok(data)=>{
                if let Some(texture) = self.get_texture(){
                    data.into_texture(cx, texture);
                }
                else{
                    self.set_texture(Some(data.into_new_texture(cx)));
                }
            }
            Err(err)=>{
                error!("load_jpg_from_data: Cannot load png image from data {}", err);
            }
        }
    }

    fn load_image_dep_by_path(
        &mut self,
        cx: &mut Cx,
        image_path: &str,
    ) {
        if let Some(texture) = cx.get_global::<ImageCache>().map.get(image_path){
            self.set_texture(Some(texture.clone()));
        }
        else{
            match cx.get_dependency(image_path) {
                Ok(data) => {
                    if image_path.ends_with(".jpg") {
                        match ImageBuffer::from_jpg(&*data){
                            Ok(data)=>{
                                let texture = data.into_new_texture(cx);
                                cx.get_global::<ImageCache>().map.insert(image_path.to_string(), texture.clone());
                                self.set_texture(Some(texture));
                            }
                            Err(err)=>{
                                error!("load_image_dep_by_path: Cannot load jpeg image from path: {} {}",image_path, err);
                            }
                        }
                    } else if image_path.ends_with(".png") {
                        match ImageBuffer::from_png(&*data){
                            Ok(data)=>{
                                let texture = data.into_new_texture(cx);
                                cx.get_global::<ImageCache>().map.insert(image_path.to_string(), texture.clone());
                                self.set_texture(Some(texture));
                            }
                            Err(err)=>{
                                error!("load_image_dep_by_path: Cannot load png image from path: {} {}",image_path, err);
                            }
                        }
                    } else {
                        error!("load_image_dep_by_path: Image format not supported {}",image_path);
                    }
                }
                Err(err) => {
                    error!("load_image_dep_by_path:  Resource not found {} {}",image_path, err);
                }
            }
        }
    }
}
