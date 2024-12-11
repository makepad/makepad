use crate::{makepad_draw::*};
use std::collections::HashMap;
use zune_jpeg::JpegDecoder;
use makepad_zune_png::{PngDecoder,post_process_image};
use std::fmt;
use std::io::prelude::*;
use std::fs::File;

pub use makepad_zune_png::error::PngDecodeErrors;
pub use zune_jpeg::errors::DecodeErrors as JpgDecodeErrors;

#[derive(Live, LiveHook)]
#[live_ignore]
pub enum ImageFit{
    #[pick] Stretch,
    Horizontal,
    Vertical,
    Smallest,
    Biggest,
    Size
}


#[derive(Default, Clone)] 
pub struct ImageBuffer {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u32>,
    pub animation: Option<TextureAnimation>,
}

impl ImageBuffer {
    pub fn new(in_data: &[u8], width: usize, height: usize) -> Result<ImageBuffer, ImageError> {
        let mut out = Vec::new();
        let pixels = width * height;
        out.resize(pixels, 0u32);
        // input pixel packing
        match in_data.len() / pixels {
            4 => for i in 0..pixels {
                let r = in_data[i*4];
                let g = in_data[i*4+1];
                let b = in_data[i*4+2];
                let a = in_data[i*4+3];
                out[i] = ((a as u32)<<24) | ((r as u32)<<16) | ((g as u32)<<8) | ((b as u32)<<0);
            }
            3 => for i in 0..pixels {
                let r = in_data[i*3];
                let g = in_data[i*3+1];
                let b = in_data[i*3+2];
                out[i] = 0xff000000 | ((r as u32)<<16) | ((g as u32)<<8) | ((b as u32)<<0);
            }
            2 => for i in 0..pixels {
                let r = in_data[i*2];
                let a = in_data[i*2+1];
                out[i] = ((a as u32)<<24) | ((r as u32)<<16) | ((r as u32)<<8) | ((r as u32)<<0);
            }
            1 => for i in 0..pixels {
                let r = in_data[i];
                out[i] = ((0xff as u32)<<24) | ((r as u32)<<16) | ((r as u32)<<8) | ((r as u32)<<0);
            }
            unsupported => {
                error!("ImageBuffer::new Image buffer pixel alignment of {unsupported} is unsupported.");
                return Err(ImageError::InvalidPixelAlignment(unsupported));
            }     
        }
        Ok(ImageBuffer {
            width,
            height,
            data: out,
            animation: None
        })
    }
    
    pub fn into_new_texture(self, cx:&mut Cx)->Texture{
        let texture = Texture::new_with_format(cx, TextureFormat::VecBGRAu8_32 {
            width: self.width,
            height: self.height,
            data: Some(self.data),
            updated: TextureUpdated::Full,
        });
        texture.set_animation(cx, self.animation);
        texture
    }
    
    pub fn from_png(
        data: &[u8]
    ) -> Result<Self, ImageError> {
        let mut decoder = PngDecoder::new(data);
        
        decoder.decode_headers().unwrap();
        
        if decoder.is_animated(){
            let colorspace = decoder.get_colorspace().unwrap();
            let info = decoder.get_info().unwrap().clone();
            let (width,height) = decoder.get_dimensions().unwrap();
            let components = colorspace.num_components();
            let mut output =
            vec![0; info.width * info.height * components];
            
            let fits_horizontal = Cx::max_texture_width() / info.width;
            let total_width = fits_horizontal * info.width;
            
            let actl_info = decoder.actl_info().unwrap();
            let total_height = ((actl_info.num_frames as usize / fits_horizontal) + 1) * info.height;
            
            let mut final_buffer = ImageBuffer::default();
            final_buffer.data.resize(total_width * total_height, 0);
            final_buffer.width = total_width;
            final_buffer.height = total_height;
            let mut cx = 0;
            let mut cy = 0;
            final_buffer.animation = Some(TextureAnimation{
                width,
                height,
                num_frames: actl_info.num_frames as usize
            });
                        
            while decoder.more_frames() {
                // decoding a video
                // decode the header, in case we haven't processed a frame header
                decoder.decode_headers().unwrap();
                // then decode the current frame information,
                // NB: Frame information is for current frame hence should be accessed before decoding the frame
                // as it will change on subsequent frames
                let frame = decoder.frame_info().unwrap();
                // decode the raw pixels, even on smaller frames, we only allocate frame_info.width*frame_info.height
                let pix = decoder.decode_raw().unwrap();
                // call post process
                post_process_image(
                    &info,
                    colorspace,
                    &frame,
                    &pix,
                    None,
                    &mut output,
                    None
                ).unwrap();
                match components {
                    3 =>  {
                        // copy output in
                        for y in 0..height{
                            for x in 0..width{
                                let r = output[y * width * 3 + x * 3 + 0];
                                let g = output[y * width * 3 + x * 3 + 1];
                                let b = output[y * width * 3 + x * 3 + 2];
                                final_buffer.data[(y+cy) * total_width + (x+cx)] = 0xff000000 | ((r as u32)<<16) | ((g as u32)<<8) | ((b as u32)<<0);
                            }
                        }
                    }
                    _=> {
                        return Err(ImageError::InvalidPixelAlignment(components));
                    }     
                }
                cx += width;
                if cx >= total_width{
                    cy += height;
                    cx = 0
                } 
            }
            return Ok(final_buffer)
        }
        else{
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
                    Err(ImageError::PngDecode(err))
                }
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


/// The possible errors that can occur when loading or creating an image texture.
#[derive(Debug)]
pub enum ImageError {
    /// The image data buffer was empty.
    EmptyData,
    /// The image's pixel data was not aligned to 3-byte or 4-byte pixels.
    /// The unsupported alignment value (in bytes) is included.
    InvalidPixelAlignment(usize),
    /// The image data could not be decoded as a JPEG.
    JpgDecode(JpgDecodeErrors),
    /// The image file at the given resource path could not be found.
    PathNotFound(String),
    /// The image data could not be decoded as a PNG.
    PngDecode(PngDecodeErrors),
    /// The image data was in an unsupported format.
    /// Currently, only JPEG and PNG are supported.
    UnsupportedFormat,
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

pub trait ImageCacheImpl {
    fn get_texture(&self, id:usize) -> &Option<Texture>;
    fn set_texture(&mut self, texture: Option<Texture>,id: usize);

    fn lazy_create_image_cache(&mut self,cx: &mut Cx) {
        if !cx.has_global::<ImageCache>() {
            cx.set_global(ImageCache::new());
        }
    }

    fn load_png_from_data(&mut self, cx: &mut Cx, data: &[u8], id:usize) -> Result<(), ImageError> {
        match ImageBuffer::from_png(&*data){
            Ok(data)=>{
                self.set_texture(Some(data.into_new_texture(cx)), id);
                Ok(())
            }
            Err(err)=>{
                Err(err)
            }
        }
    }
    
    fn load_jpg_from_data(&mut self, cx: &mut Cx, data: &[u8], id:usize) -> Result<(), ImageError> {
        match ImageBuffer::from_jpg(&*data){
            Ok(data)=>{
                self.set_texture(Some(data.into_new_texture(cx)), id);
                Ok(())
            }
            Err(err)=>{
                Err(err)
            }
        }
    }
    
    fn load_image_file_by_path(
        &mut self,
        cx: &mut Cx,
        image_path: &str,
        id: usize,
    ) -> Result<(), ImageError> {
        log!("LOADING FROM DISK  {}", image_path);
        if let Some(texture) = cx.get_global::<ImageCache>().map.get(image_path){
            self.set_texture(Some(texture.clone()), id);
            Ok(())
        }
        else{
            if let Ok(mut f) = File::open(image_path){
                let mut data = Vec::new();
                match f.read_to_end(&mut data) {
                    Ok(_len) => {
                        if image_path.ends_with(".jpg") {
                            match ImageBuffer::from_jpg(&*data){
                                Ok(data)=>{
                                    let texture = data.into_new_texture(cx);
                                    cx.get_global::<ImageCache>().map.insert(image_path.to_string(), texture.clone());
                                    self.set_texture(Some(texture), id);
                                    Ok(())
                                }
                                Err(err)=>{
                                    error!("load_image_file_by_path: Cannot load jpeg image from path: {} {}", image_path, err);
                                    Err(err)
                                }
                            }
                        } else if image_path.ends_with(".png") {
                            match ImageBuffer::from_png(&*data){
                                Ok(data)=>{
                                    let texture = data.into_new_texture(cx);
                                    cx.get_global::<ImageCache>().map.insert(image_path.to_string(), texture.clone());
                                    self.set_texture(Some(texture), id);
                                    Ok(())
                                }
                                Err(err)=>{
                                    error!("load_image_file_by_path: Cannot load png image from path: {} {}", image_path, err);
                                    Err(err)
                                }
                            }
                        } else {
                            error!("load_image_file_by_path: Image format not supported {}", image_path);
                            Err(ImageError::UnsupportedFormat)
                        }
                    }
                    Err(err) => {
                        error!("load_image_file_by_path: Resource not found {} {}", image_path, err);
                        Err(ImageError::PathNotFound(image_path.to_string()))
                    }
                }
            }
            else{
                error!("load_image_file_by_path: File not found {}", image_path);
                Err(ImageError::PathNotFound(image_path.to_string()))
            }
        }
    }
    
    fn load_image_dep_by_path(
        &mut self,
        cx: &mut Cx,
        image_path: &str,
        id: usize,
    ) -> Result<(), ImageError> {
        if let Some(texture) = cx.get_global::<ImageCache>().map.get(image_path){
            self.set_texture(Some(texture.clone()), id);
            Ok(())
        } 
        else{
            match cx.take_dependency(image_path) {
                Ok(data) => {
                    if image_path.ends_with(".jpg") {
                        match ImageBuffer::from_jpg(&*data){
                            Ok(data)=>{
                                let texture = data.into_new_texture(cx);
                                cx.get_global::<ImageCache>().map.insert(image_path.to_string(), texture.clone());
                                self.set_texture(Some(texture), id);
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
                                self.set_texture(Some(texture), id);
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
