use crate::{makepad_draw::*, ImageError};
use std::collections::HashMap;
use makepad_zune_jpeg::JpegDecoder;
use makepad_zune_png::PngDecoder;

pub use makepad_zune_png::error::PngDecodeErrors;
pub use makepad_zune_jpeg::errors::DecodeErrors as JpgDecodeErrors;

#[derive(Live, LiveHook)]
#[live_ignore]
pub enum ImageFit{
    #[pick] Stretch,
    Horizontal,
    Vertical,
    Smallest,
    Biggest
}


#[derive(Default, Clone)] 
pub struct ImageBuffer {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u32>,
}

impl ImageBuffer {
    pub fn new(in_data: &[u8], width: usize, height: usize) -> Result<ImageBuffer, ImageError> {
        let mut out = Vec::new();
        let pixels = width * height;
        out.resize(pixels, 0u32);
        // input pixel packing
        match in_data.len() / pixels {
            3 => for i in 0..pixels {
                let r = in_data[i*3];
                let g = in_data[i*3+1];
                let b = in_data[i*3+2];
                out[i] = 0xff000000 | ((r as u32)<<16) | ((g as u32)<<8) | ((b as u32)<<0);
            }
            4 => for i in 0..pixels {
                let r = in_data[i*4];
                let g = in_data[i*4+1];
                let b = in_data[i*4+2];
                let a = in_data[i*4+3];
                out[i] = ((a as u32)<<24) | ((r as u32)<<16) | ((g as u32)<<8) | ((b as u32)<<0);
            }
            unsupported => {
                error!("ImageBuffer::new Image buffer pixel alignment of {unsupported} is unsupported; must be 3 or 4");
                return Err(ImageError::InvalidPixelAlignment(unsupported));
            }
        }
        Ok(ImageBuffer {
            width,
            height,
            data: out
        })
    }
    
    pub fn into_new_texture(self, cx:&mut Cx)->Texture{
        let texture = Texture::new_with_format(cx, TextureFormat::VecBGRAu8_32 {
            width: self.width,
            height: self.height,
            data: self.data
        });
        texture
    }
    
    pub fn from_png(
        data: &[u8]
    ) -> Result<Self, ImageError> {
        let mut decoder = PngDecoder::new(data);
        match decoder.decode() {
            Ok(image) => {
                if let Some(data) = image.u8() {
                    let (width,height) = decoder.get_dimensions().unwrap();
                    ImageBuffer::new(&data, width as usize, height as usize)
                }
                else{
                    error!("Error decoding PNG: image data empty");
                    Err(ImageError::EmptyData)
                }
            }
            Err(err) => {
                error!("Error decoding PNG: {:?}", err);
                Err(ImageError::PngDecode(err))
            }
        }
    }

    pub fn from_jpg(
        data: &[u8]
    ) -> Result<Self, ImageError> {
        let mut decoder = JpegDecoder::new(&*data);
        // decode the file
        match decoder.decode() {
            Ok(data) => {
                let info = decoder.info().unwrap();
                ImageBuffer::new(&data, info.width as usize, info.height as usize)
            },
            Err(err) => {
                error!("Error decoding JPG: {:?}", err);
                Err(ImageError::JpgDecode(err))
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

    fn load_png_from_data(&mut self, cx: &mut Cx, data: &[u8]) -> Result<(), ImageError> {
        match ImageBuffer::from_png(&*data){
            Ok(data)=>{
                self.set_texture(Some(data.into_new_texture(cx)));
                Ok(())
            }
            Err(err)=>{
                error!("load_png_from_data: Cannot load png image from data: {}", err);
                Err(err)
            }
        }
    }
    
    fn load_jpg_from_data(&mut self, cx: &mut Cx, data: &[u8]) -> Result<(), ImageError> {
        match ImageBuffer::from_jpg(&*data){
            Ok(data)=>{
                self.set_texture(Some(data.into_new_texture(cx)));
                Ok(())
            }
            Err(err)=>{
                error!("load_jpg_from_data: Cannot load png image from data: {}", err);
                Err(err)
            }
        }
    }

    fn load_image_dep_by_path(
        &mut self,
        cx: &mut Cx,
        image_path: &str,
    ) -> Result<(), ImageError> {
        if let Some(texture) = cx.get_global::<ImageCache>().map.get(image_path){
            self.set_texture(Some(texture.clone()));
            Ok(())
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
                                Ok(())
                            }
                            Err(err)=>{
                                error!("load_image_dep_by_path: Cannot load jpeg image from path: {} {}", image_path, err);
                                Err(err)
                            }
                        }
                    } else if image_path.ends_with(".png") {
                        match ImageBuffer::from_png(&*data){
                            Ok(data)=>{
                                let texture = data.into_new_texture(cx);
                                cx.get_global::<ImageCache>().map.insert(image_path.to_string(), texture.clone());
                                self.set_texture(Some(texture));
                                Ok(())
                            }
                            Err(err)=>{
                                error!("load_image_dep_by_path: Cannot load png image from path: {} {}", image_path, err);
                                Err(err)
                            }
                        }
                    } else {
                        error!("load_image_dep_by_path: Image format not supported {}", image_path);
                        Err(ImageError::UnsupportedFormat)
                    }
                }
                Err(err) => {
                    error!("load_image_dep_by_path: Resource not found {} {}", image_path, err);
                    Err(ImageError::PathNotFound(image_path.to_string()))
                }
            }
        }
    }
}
