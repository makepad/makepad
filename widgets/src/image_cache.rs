use crate::makepad_draw::*;
use std::collections::HashMap;
use std::error::Error;
use zune_jpeg::JpegDecoder;
use makepad_zune_png::{post_process_image, PngDecoder};
use std::fmt;
use std::io::prelude::*;
use std::fs::File;
use std::path::{Path,PathBuf};
use std::cell::RefCell;

pub use makepad_zune_png::error::PngDecodeErrors;
pub use zune_jpeg::errors::DecodeErrors as JpgDecodeErrors;

#[derive(Live, LiveHook, Clone, Copy)]
#[live_ignore]
pub enum ImageFit {
    #[pick] Stretch,
    Horizontal,
    Vertical,
    Smallest,
    Biggest,
    Size
}

impl Default for ImageFit {
    fn default() -> Self {
        ImageFit::Stretch
    }
}

#[derive(Debug, Default, Clone)] 
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
    
    pub fn from_png(data: &[u8]) -> Result<Self, ImageError> {
        let mut decoder = PngDecoder::new(data);
        decoder.decode_headers()?;
        
        if decoder.is_animated() {
            return Ok(Self::decode_animated_png(&mut decoder)?);
        }

        let image = decoder.decode()?;
        let decoded_data = image.u8().ok_or(
            ImageError::PngDecode(PngDecodeErrors::GenericStatic(
                "Failed to decode PNG image data as a slice of u8 bytes"
            )),
        )?;
        let (width, height) = decoder.get_dimensions().ok_or(
            ImageError::PngDecode(PngDecodeErrors::GenericStatic(
                "Failed to get PNG image dimensions"
            ))
        )?;
        Self::new(&decoded_data, width, height)
    }

    fn decode_animated_png(decoder: &mut PngDecoder<&[u8]>) -> Result<ImageBuffer, ImageError> {
        let colorspace = decoder.get_colorspace().ok_or(
            ImageError::PngDecode(PngDecodeErrors::GenericStatic(
                "Failed to get animated PNG colorspace"
            ))
        )?;
        let (width, height) = decoder.get_dimensions().ok_or(
            ImageError::PngDecode(PngDecodeErrors::GenericStatic(
                "Failed to get animated PNG image dimensions"
            ))
        )?;
        let actl_info = decoder.actl_info().ok_or(
            ImageError::PngDecode(PngDecodeErrors::GenericStatic(
                "Failed to get animated PNG actl info"
            ))
        )?;

        let num_components = colorspace.num_components();
        let mut output = vec![0; width * height * num_components];
        let fits_horizontal = Cx::max_texture_width() / width;
        let total_width = fits_horizontal * width;
        let total_height = ((actl_info.num_frames as usize / fits_horizontal) + 1) * height;
        let mut final_buffer = ImageBuffer::default();
        final_buffer.data.resize(total_width * total_height, 0);
        final_buffer.width = total_width;
        final_buffer.height = total_height;
        let mut cx = 0;
        let mut cy = 0;
        final_buffer.animation = Some(TextureAnimation {
            width,
            height,
            num_frames: actl_info.num_frames as usize
        });
        let mut previous_frame = None;
        while decoder.more_frames() {
            // decoding a video
            // decode the header, in case we haven't processed a frame header
            decoder.decode_headers()?;
            // then decode the current frame information,
            // NB: Frame information is for current frame hence should be accessed before decoding the frame
            // as it will change on subsequent frames
            let frame = decoder.frame_info().expect("to have already been decoded");
            // decode the raw pixels, even on smaller frames, we only allocate frame_info.width*frame_info.height
            let pix = decoder.decode_raw()?;
            // Get the PNG image info here instead of outside the loop, which prevents borrow checker errors.
            // It is way more efficient to do this here instead of to clone the PngInfo outside of this loop.
            let info = decoder.get_info().ok_or(
                ImageError::PngDecode(PngDecodeErrors::GenericStatic(
                    "Failed to get animated PNG image info"
                ))
            )?;
            // call post process
            post_process_image(
                &info,
                colorspace,
                &frame,
                &pix,
                previous_frame.as_deref(),
                &mut output,
                None
            )?;
            previous_frame = Some(pix);
            match num_components {
                4 => {
                    for y in 0..height {
                        for x in 0..width {
                            let r = output[y * width * 4 + x * 4 + 0];
                            let g = output[y * width * 4 + x * 4 + 1];
                            let b = output[y * width * 4 + x * 4 + 2];
                            let a = output[y * width * 4 + x * 4 + 3];
                            final_buffer.data[(y+cy) * total_width + (x+cx)] = ((a as u32)<<24) | ((r as u32)<<16) | ((g as u32)<<8) | ((b as u32)<<0);
                        }
                    }
                }
                3 => {
                    for y in 0..height {
                        for x in 0..width {
                            let r = output[y * width * 3 + x * 3 + 0];
                            let g = output[y * width * 3 + x * 3 + 1];
                            let b = output[y * width * 3 + x * 3 + 2];
                            final_buffer.data[(y+cy) * total_width + (x+cx)] = 0xff000000 | ((r as u32)<<16) | ((g as u32)<<8) | ((b as u32)<<0);
                        }
                    }
                }
                _ => {
                    return Err(ImageError::InvalidPixelAlignment(num_components));
                }     
            }
            cx += width;
            if cx >= total_width {
                cy += height;
                cx = 0
            } 
        }
        Ok(final_buffer)
    }

    pub fn from_jpg(data: &[u8]) -> Result<Self, ImageError> {
        let mut decoder = JpegDecoder::new(&*data);
        match decoder.decode() {
            Ok(data) => {
                let info = decoder.info().ok_or(
                    ImageError::JpgDecode(JpgDecodeErrors::FormatStatic(
                        "Failed to decode JPG image info"
                    )),
                )?;
                ImageBuffer::new(&data, info.width as usize, info.height as usize)
            },
            Err(err) => Err(ImageError::JpgDecode(err)),
        }
    }
}

pub enum ImageCacheEntry{
    Loaded(Texture),
    Loading(usize, usize),
}

#[derive(Debug)]
pub struct AsyncImageLoad{
    pub image_path: PathBuf,
    pub result: RefCell<Option<Result<ImageBuffer, ImageError>>>
}

pub struct ImageCache {
    map: HashMap<PathBuf, ImageCacheEntry>,
    pub thread_pool: Option<TagThreadPool<PathBuf>>,
}

impl ImageCache {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            thread_pool: None,
        }
    }
}


/// The possible errors that can occur when loading or creating an image texture.
#[derive(Debug)]
pub enum ImageError {
    /// The image data buffer was empty or otherwise invalid.
    EmptyData,
    /// The image's pixel data was not aligned to 3-byte or 4-byte pixels.
    /// The unsupported alignment value (in bytes) is included.
    InvalidPixelAlignment(usize),
    /// The image data could not be decoded as a JPEG.
    JpgDecode(JpgDecodeErrors),
    /// The image file at the given resource path could not be found.
    PathNotFound(PathBuf),
    /// The image data could not be decoded as a PNG.
    PngDecode(PngDecodeErrors),
    /// The image data was in an unsupported format.
    /// Currently, only JPEG and PNG are supported.
    UnsupportedFormat,
}

pub enum AsyncLoadResult{
    Loading(usize,usize),
    Loaded,
}

impl Error for ImageError {}

impl From<PngDecodeErrors> for ImageError {
    fn from(value: PngDecodeErrors) -> Self {
        Self::PngDecode(value)
    }
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
    
    fn image_size_by_path(image_path:&Path)-> Result<(usize,usize), ImageError> {
        if let Ok(mut f) = File::open(image_path){
            let mut data = vec![0u8;1024]; // yolo chunk size
            match f.read(&mut data) {
                Ok(_len) => {
                    if image_path.extension().map(|s| s == "jpg").unwrap_or(false) {
                        let mut decoder = JpegDecoder::new(&*data);
                        decoder.decode_headers().unwrap();
                        let image_info = decoder.info().unwrap();
                        return Ok((image_info.width as usize,image_info.height as usize))
                    } else if image_path.extension().map(|s| s == "png").unwrap_or(false) {
                        let mut decoder = PngDecoder::new(data);
                        decoder.decode_headers()?;
                        let (width,height) = decoder.get_dimensions().ok_or(
                            ImageError::PngDecode(PngDecodeErrors::GenericStatic(
                                "Failed to get animated PNG image dimensions"
                            ))
                        )?;
                        return Ok((width,height))
                                                
                    } else {
                        return Err(ImageError::UnsupportedFormat)
                    }
                }
                Err(err) => {
                    error!("load_image_file_by_path: Resource not found {:?} {}", image_path, err);
                    return Err(ImageError::PathNotFound(image_path.into()))
                }
            }
        }
        else{
            error!("load_image_file_by_path: File not found {:?}", image_path);
            return Err(ImageError::PathNotFound(image_path.into()))
        }
    }
        
    fn process_async_image_load(&mut self, cx:&mut Cx, image_path: &Path, result: Result<ImageBuffer, ImageError>)->bool{
        // alright now we should stuff this thing into our cache
        if let Ok(data) = result{
            let texture = data.into_new_texture(cx);
            cx.get_global::<ImageCache>().map.insert(image_path.into(), ImageCacheEntry::Loaded(texture.clone()));
        }
        false
    }
    
    fn load_image_from_cache(&mut self, cx:&mut Cx, image_path: &Path, id: usize)->bool{
        if let Some(texture) = cx.get_global::<ImageCache>().map.get(image_path){
            match texture{
                ImageCacheEntry::Loaded(texture)=>{
                    self.set_texture(Some(texture.clone()), id);
                    return true
                }
                _=>()
            }
        }
        false
    }
    
    fn load_image_file_by_path_async_impl(
        &mut self,
        cx: &mut Cx,
        image_path: &Path,
        id: usize,
    ) -> Result<AsyncLoadResult, ImageError> {
        if let Some(texture) = cx.get_global::<ImageCache>().map.get(image_path){
            match texture{
                ImageCacheEntry::Loaded(texture)=>{
                    let texture = texture.clone();
                    // lets fetch the texture size
                    //let (_w,_h) = texture.get_format(cx).vec_width_height().unwrap_or((100,100));
                    self.set_texture(Some(texture), id);
                    Ok(AsyncLoadResult::Loaded)
                }
                ImageCacheEntry::Loading(w,h)=>{
                    Ok(AsyncLoadResult::Loading(*w, *h))
                }
            }
        }
        else{
            if  cx.get_global::<ImageCache>().thread_pool.is_none(){
                 cx.get_global::<ImageCache>().thread_pool = Some(TagThreadPool::new(cx, cx.cpu_cores().max(3) - 2))
            }
            let (w,h) = Self::image_size_by_path(image_path)?;
            // open image file and read the headers
            cx.get_global::<ImageCache>().map.insert(image_path.into(), ImageCacheEntry::Loading(w,h));
            
            cx.get_global::<ImageCache>().thread_pool.as_mut().unwrap().execute_rev(image_path.into(), move |image_path|{
                if let Ok(mut f) = File::open(&image_path){
                    let mut data = Vec::new();
                    match f.read_to_end(&mut data) {
                        Ok(_len) => {        
                            if image_path.extension().map(|s| s == "jpg").unwrap_or(false) {
                                match ImageBuffer::from_jpg(&*data){
                                    Ok(data)=>{
                                        Cx::post_action(AsyncImageLoad{
                                            image_path, 
                                            result: RefCell::new(Some(Ok(data)))
                                        });
                                    }
                                    Err(err)=>{
                                        Cx::post_action(AsyncImageLoad{
                                            image_path, 
                                            result: RefCell::new(Some(Err(err)))
                                        });
                                    }
                                }
                            } else if image_path.extension().map(|s| s == "png").unwrap_or(false) {
                                log!("LOADIN4");
                                
                                match ImageBuffer::from_png(&*data){
                                    Ok(data)=>{
                                        Cx::post_action(AsyncImageLoad{
                                            image_path, 
                                            result: RefCell::new(Some(Ok(data)))
                                        });
                                    }
                                    Err(err)=>{
                                        Cx::post_action(AsyncImageLoad{
                                            image_path, 
                                            result: RefCell::new(Some(Err(err)))
                                        });
                                    }
                                }
                            } else {
                                Cx::post_action(AsyncImageLoad{
                                    image_path, 
                                    result: RefCell::new(Some(Err(ImageError::UnsupportedFormat)))
                                });
                            }
                        }
                        Err(_err) => {
                            Cx::post_action(AsyncImageLoad{
                                image_path: image_path.clone(), 
                                result: RefCell::new(Some(Err(ImageError::PathNotFound(image_path))))
                            });
                        }
                    }
                }
                else{
                    Cx::post_action(AsyncImageLoad{
                        image_path: image_path.clone(), 
                        result: RefCell::new(Some(Err(ImageError::PathNotFound(image_path))))
                    });
                }
            });
            Ok(AsyncLoadResult::Loading(w, h))
        }
    }
    
    fn load_image_file_by_path_and_data(&mut self, cx:&mut Cx, data:&[u8], id:usize, image_path:&Path)-> Result<(), ImageError> {
        if image_path.extension().map(|s| s == "jpg").unwrap_or(false) {
            match ImageBuffer::from_jpg(&*data){
                Ok(data)=>{
                    let texture = data.into_new_texture(cx);
                    cx.get_global::<ImageCache>().map.insert(image_path.into(), ImageCacheEntry::Loaded(texture.clone()));
                    self.set_texture(Some(texture), id);
                    Ok(())
                }
                Err(err)=>{
                    error!("load_image_file_by_path_and_data: Cannot load jpeg image from path: {:?} {}", image_path, err);
                    Err(err)
                }
            }
        } else if image_path.extension().map(|s| s == "png").unwrap_or(false) {
            match ImageBuffer::from_png(&*data){
                Ok(data)=>{
                    let texture = data.into_new_texture(cx);
                    cx.get_global::<ImageCache>().map.insert(image_path.into(), ImageCacheEntry::Loaded(texture.clone()));
                    self.set_texture(Some(texture), id);
                    Ok(())
                }
                Err(err)=>{
                    error!("load_image_file_by_path_and_data: Cannot load png image from path: {:?} {}", image_path, err);
                    Err(err)
                }
            }
        } else {
            error!("load_image_file_by_path_and_data: Image format not supported {:?}", image_path);
            Err(ImageError::UnsupportedFormat)
        }
    }
        
    fn load_image_file_by_path(
        &mut self,
        cx: &mut Cx,
        image_path: &Path,
        id: usize,
    ) -> Result<(), ImageError> {
        if let Some(ImageCacheEntry::Loaded(texture)) = cx.get_global::<ImageCache>().map.get(image_path){
            self.set_texture(Some(texture.clone()), id);
            Ok(())
        }
        else{
            if let Ok(mut f) = File::open(image_path){
                let mut data = Vec::new();
                match f.read_to_end(&mut data) {
                    Ok(_len) => {
                        self.load_image_file_by_path_and_data(cx, &data, id, image_path)
                    }
                    Err(err) => {
                        error!("load_image_file_by_path: Resource not found {:?} {}", image_path, err);
                        Err(ImageError::PathNotFound(image_path.into()))
                    }
                }
            }
            else{
                error!("load_image_file_by_path: File not found {:?}", image_path);
                Err(ImageError::PathNotFound(image_path.into()))
            }
        }
    }
    
    fn load_image_dep_by_path(
        &mut self,
        cx: &mut Cx,
        image_path: &str,
        id: usize,
    ) -> Result<(), ImageError> {
        let p_image_path = Path::new(image_path);
        if let Some(ImageCacheEntry::Loaded(texture)) = cx.get_global::<ImageCache>().map.get(p_image_path){
            self.set_texture(Some(texture.clone()), id);
            Ok(())
        } 
        else{
            match cx.take_dependency(image_path) {
                Ok(data) => {
                    self.load_image_file_by_path_and_data(cx, &data, id, p_image_path)
                }
                Err(err) => {
                    error!("load_image_dep_by_path: Resource not found {} {}", image_path, err);
                    Err(ImageError::PathNotFound(image_path.into()))
                }
            }
        }
    }
}
