use {
    crate::{
        id_pool::*,
        cx::Cx,
        makepad_math::*,
        os::CxOsTexture,
    },
    std::rc::Rc,
};


#[derive(Debug, Clone)]
pub struct Texture(Rc<PoolId>);

#[derive(Clone, Debug, PartialEq, Copy)]
pub struct TextureId(pub (crate) usize, u64);

impl Texture {
    pub fn texture_id(&self) -> TextureId {TextureId(self.0.id, self.0.generation)}
}

#[derive(Default)]
pub struct CxTexturePool(pub (crate) IdPool<CxTexture>);

impl CxTexturePool {
    // Allocates a new texture in the pool, potentially reusing an existing texture slot.
    ///
    /// This method attempts to find a compatible texture slot for reuse. If found, it preserves
    /// the old os-specific resources for proper cleanup. If not, it allocates a new slot.
    ///
    /// # Arguments
    /// * `requested_format` - The format of the texture to be allocated.
    ///
    /// # Returns
    /// A `Texture` instance representing the allocated or reused texture.
    ///
    /// # Note
    /// When a texture slot is reused, the old platofrm-specific resources are stored in the `previous_platform_resource` field
    /// of the new `CxTexture`. This allows for proper resource management and cleanup in the corresponding platform.
    pub fn alloc(&mut self, requested_format: TextureFormat) -> Texture {
        let is_video = requested_format.is_video();
        let cx_texture = CxTexture {
            format: requested_format,
            alloc: None,
            ..Default::default()
        };

        let (new_id, previous_item) = self.0.alloc_with_reuse_filter(|item| {
            // Check for compatibility, intentionally not using `is_compatible_with` to avoid passing the whole format and cloning vec contents    
            is_video == item.item.format.is_video()
        }, cx_texture);

        if let Some(previous_item) = previous_item {
            // We know this index is valid because it was just reused
            self.0.pool[new_id.id].item.previous_platform_resource = Some(previous_item.os);
        }

        Texture(Rc::new(new_id))
    }
}

impl std::ops::Index<TextureId> for CxTexturePool {
    type Output = CxTexture;
    fn index(&self, index: TextureId) -> &Self::Output {
        let d = &self.0.pool[index.0];
        if d.generation != index.1 {
            error!("Texture id generation wrong {} {} {}", index.0, d.generation, index.1)
        }
        &d.item
    }
}

impl std::ops::IndexMut<TextureId> for CxTexturePool {
    fn index_mut(&mut self, index: TextureId) -> &mut Self::Output {
        let d = &mut self.0.pool[index.0];
        if d.generation != index.1 {
            error!("Texture id generation wrong {} {} {}", index.0, d.generation, index.1)
        }
        &mut d.item
    }
}


#[derive(Clone, Debug)]
pub enum TextureSize {
    Auto,
    Fixed{width: usize, height: usize}
}

impl TextureSize{
    fn width_height(&self, w:usize, h:usize)->(usize,usize){
        match self{
            TextureSize::Auto=>(w,h),
            TextureSize::Fixed{width, height}=>(*width,*height)
        }
    }
}


#[derive(Clone, Debug)]
pub enum TextureFormat {
    Unknown,
    VecBGRAu8_32{width:usize, height:usize, data:Option<Vec<u32>>, updated: TextureUpdated},
    VecMipBGRAu8_32{width:usize, height:usize, data:Option<Vec<u32>>, max_level:Option<usize>, updated: TextureUpdated},
    VecRGBAf32{width:usize, height:usize, data:Option<Vec<f32>>, updated: TextureUpdated},
    VecRu8{width:usize, height:usize, data:Option<Vec<u8>>, unpack_row_length:Option<usize>, updated: TextureUpdated},
    VecRGu8{width:usize, height:usize, data:Option<Vec<u8>>, unpack_row_length:Option<usize>, updated: TextureUpdated},
    VecRf32{width:usize, height:usize, data:Option<Vec<f32>>, updated: TextureUpdated},
    DepthD32{size:TextureSize, initial: bool},
    RenderBGRAu8{size:TextureSize, initial: bool},
    RenderRGBAf16{size:TextureSize, initial: bool},
    RenderRGBAf32{size:TextureSize, initial: bool},
    SharedBGRAu8{width:usize, height:usize, id:crate::cx_stdin::PresentableImageId, initial: bool},
    #[cfg(any(target_os = "android", target_os = "linux"))]
    VideoRGB,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TextureAlloc{
    pub category: TextureCategory,
    pub pixel: TexturePixel,
    pub width: usize,
    pub height: usize,
}

#[allow(unused)]    
#[derive(Clone, Debug)]
pub enum TextureCategory{
    Vec,
    Render,
    DepthBuffer,
    Shared,
    Video,
}

impl PartialEq for TextureCategory{
    fn eq(&self, other: &TextureCategory) -> bool{
        match self{
            Self::Vec{..} => if let Self::Vec{..} = other{true} else {false},
            Self::Render{..} => if let Self::Render{..} = other{true} else {false},
            Self::Shared{..} => if let Self::Shared{..} = other{true} else {false},
            Self::DepthBuffer{..} => if let Self::DepthBuffer{..} = other{true} else {false},           
            Self::Video{..} => if let Self::Video{..} = other{true} else {false},           
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum TextureUpdated {
    Empty,
    Partial(RectUsize),
    Full,
}

impl TextureUpdated {
    pub fn is_empty(&self) -> bool {
        match self {
            TextureUpdated::Empty => true,
            _ => false,
        }
    }

    pub fn update(self, dirty_rect: Option<RectUsize>) -> Self {
        match dirty_rect {
            Some(dirty_rect) => match self {
                TextureUpdated::Empty => TextureUpdated::Partial(dirty_rect),
                TextureUpdated::Partial(rect) => TextureUpdated::Partial(rect.union(dirty_rect)),
                TextureUpdated::Full => TextureUpdated::Full,
            },
            None => TextureUpdated::Full,
        }
    }
}

#[allow(unused)]    
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TexturePixel{
    BGRAu8,
    RGBAf16,
    RGBAf32,
    Ru8,
    RGu8,
    Rf32,
    D32,
    #[cfg(any(target_os = "android", target_os = "linux"))]
    VideoRGB
}

impl CxTexture{
    pub(crate) fn updated(&self) -> TextureUpdated {
        match self.format {
            TextureFormat::VecBGRAu8_32 { updated, .. } => updated,
            TextureFormat::VecMipBGRAu8_32{ updated, .. } => updated,
            TextureFormat::VecRGBAf32 { updated, .. } => updated,
            TextureFormat::VecRu8 { updated, .. } => updated,
            TextureFormat::VecRGu8 { updated, .. } => updated,
            TextureFormat::VecRf32 { updated, .. } => updated,
            _ => panic!(),
        }
    }

    pub(crate) fn initial(&mut self) -> bool {
        match self.format {
            TextureFormat::DepthD32{ initial, .. } => initial,
            TextureFormat::RenderBGRAu8{ initial, .. } => initial,
            TextureFormat::RenderRGBAf16{ initial, .. } => initial,
            TextureFormat::RenderRGBAf32{ initial, .. } => initial,
            TextureFormat::SharedBGRAu8{ initial, .. } => initial,
            _ => panic!()
        }
    }

    pub(crate) fn set_updated(&mut self, updated: TextureUpdated) {
        *match &mut self.format {
            TextureFormat::VecBGRAu8_32 { updated, .. } => updated,
            TextureFormat::VecMipBGRAu8_32{ updated, .. } => updated,
            TextureFormat::VecRGBAf32 { updated, .. } => updated,
            TextureFormat::VecRu8 { updated, .. } => updated,
            TextureFormat::VecRGu8 { updated, .. } => updated,
            TextureFormat::VecRf32 { updated, .. } => updated,
            _ => panic!(),
        } = updated;
    }

    pub fn set_initial(&mut self, initial: bool) {
        *match &mut self.format {
            TextureFormat::DepthD32{ initial, .. } => initial,
            TextureFormat::RenderBGRAu8{ initial, .. } => initial,
            TextureFormat::RenderRGBAf16{ initial, .. } => initial,
            TextureFormat::RenderRGBAf32{ initial, .. } => initial,
            TextureFormat::SharedBGRAu8{ initial, .. } => initial,
            _ => panic!()
        } = initial;
    }

    pub(crate) fn take_updated(&mut self) -> TextureUpdated {
        let updated = self.updated();
        self.set_updated(TextureUpdated::Empty);
        updated
    }
    
    pub(crate) fn take_initial(&mut self) -> bool {
        let initial = self.initial();
        self.set_initial(false);
        initial
    }
        
    pub(crate) fn alloc_vec(&mut self)->bool{
        if let Some(alloc) = self.format.as_vec_alloc(){
            if self.alloc.is_none() || self.alloc.as_ref().unwrap() != &alloc{
                self.alloc = Some(alloc);
                return true;
            }
        }
        false
    }
    
    #[allow(unused)]
    pub(crate) fn alloc_shared(&mut self)->bool{
        if let Some(alloc) = self.format.as_shared_alloc(){
            if self.alloc.is_none() || self.alloc.as_ref().unwrap() != &alloc{
                self.alloc = Some(alloc);
                return true;
            }
        }
        false
    }
    
    pub(crate) fn alloc_render(&mut self, width:usize, height: usize)->bool{
        if let Some(alloc) = self.format.as_render_alloc(width, height){
            if self.alloc.is_none() || self.alloc.as_ref().unwrap() != &alloc{
                self.alloc = Some(alloc);
                return true;
            }
        }
        false
    }
    
    pub(crate) fn alloc_depth(&mut self, width:usize, height: usize)->bool{
        if let Some(alloc) = self.format.as_depth_alloc(width, height){
            if self.alloc.is_none() || self.alloc.as_ref().unwrap() != &alloc{
                self.alloc = Some(alloc);
                return true;
            }
        }
        false
    }

    #[cfg(any(target_os = "android", target_os = "linux"))]
    #[allow(unused)]
    pub(crate) fn alloc_video(&mut self)->bool{
        if let Some(alloc) = self.format.as_video_alloc(){
            if self.alloc.is_none() || self.alloc.as_ref().unwrap() != &alloc{
                self.alloc = Some(alloc);
                return true;
            }
        }
        false
    }
}

impl TextureFormat{
    pub fn is_shared(&self)->bool{
         match self{
             Self::SharedBGRAu8{..}=>true,
             _=>false
         }
    }
    pub fn is_vec(&self)->bool{
        match self{
            Self::VecBGRAu8_32{..}=>true,
            Self::VecMipBGRAu8_32{..}=>true,
            Self::VecRGBAf32{..}=>true,
            Self::VecRu8{..}=>true,
            Self::VecRGu8{..}=>true,
            Self::VecRf32{..}=>true,
            _=>false
        }
    }
    
    pub fn is_render(&self)->bool{
        match self{
            Self::RenderBGRAu8{..}=>true,
            Self::RenderRGBAf16{..}=>true,
            Self::RenderRGBAf32{..}=>true,
            _=>false
        }
    }
        
    pub fn is_depth(&self)->bool{
        match self{
            Self::DepthD32{..}=>true,
            _=>false
        }
    }

    pub fn is_video(&self) -> bool {
        #[cfg(any(target_os = "android", target_os = "linux"))]
        if let Self::VideoRGB = self {
            return true;
        }
        false
    }

    pub fn vec_width_height(&self)->Option<(usize,usize)>{
        match self{
            Self::VecBGRAu8_32{width, height, .. }=>Some((*width,*height)),
            Self::VecMipBGRAu8_32{width, height, ..}=>Some((*width,*height)),
            Self::VecRGBAf32{width, height, ..}=>Some((*width,*height)),
            Self::VecRu8{width, height, ..}=>Some((*width,*height)),
            Self::VecRGu8{width, height, ..}=>Some((*width,*height)),
            Self::VecRf32{width, height,..}=>Some((*width,*height)),
            _=>None
        }
    }
    
    pub(crate) fn as_vec_alloc(&self)->Option<TextureAlloc>{
        match self{
            Self::VecBGRAu8_32{width,height,..}=>Some(TextureAlloc{
                width:*width,
                height:*height,
                pixel:TexturePixel::BGRAu8,
                category: TextureCategory::Vec,
            }),
            Self::VecMipBGRAu8_32{width,height,..}=>Some(TextureAlloc{
                width:*width,
                height:*height,
                pixel:TexturePixel::BGRAu8,
                category: TextureCategory::Vec,
            }),
            Self::VecRGBAf32{width,height,..}=>Some(TextureAlloc{
                width:*width,
                height:*height,
                pixel:TexturePixel::RGBAf32,
                category: TextureCategory::Vec,
            }),
            Self::VecRu8{width,height,..}=>Some(TextureAlloc{
                width:*width,
                height:*height,
                pixel:TexturePixel::Ru8,
                category: TextureCategory::Vec,
            }),
            Self::VecRGu8{width,height,..}=>Some(TextureAlloc{
                width:*width,
                height:*height,
                pixel:TexturePixel::RGu8,
                category: TextureCategory::Vec,
            }),
            Self::VecRf32{width,height,..}=>Some(TextureAlloc{
                width:*width,
                height:*height,
                pixel:TexturePixel::Rf32,
                category: TextureCategory::Vec,
            }),
            _=>None
        }
    }
    #[allow(unused)]    
    pub(crate) fn as_render_alloc(&self, width:usize, height:usize)->Option<TextureAlloc>{
        match self{
            Self::RenderBGRAu8{size,..}=>{
                let (width,height) = size.width_height(width, height);
                Some(TextureAlloc{
                    width,
                    height,
                    pixel:TexturePixel::BGRAu8,
                    category: TextureCategory::Render,
                })
            }
            Self::RenderRGBAf16{size,..}=>{
                let (width,height) = size.width_height(width, height);
                Some(TextureAlloc{
                    width,
                    height,
                    pixel:TexturePixel::RGBAf16,
                    category: TextureCategory::Render,
                })
            }
            Self::RenderRGBAf32{size,..}=>{
                let (width,height) = size.width_height(width, height);
                Some(TextureAlloc{
                    width,
                    height,
                    pixel:TexturePixel::RGBAf32,
                    category: TextureCategory::Render,
                })
            }
            _=>None
        }
    }
        
    pub(crate) fn as_depth_alloc(&self, width:usize, height:usize)->Option<TextureAlloc>{
        match self{
            Self::DepthD32{size,..}=>{
                let (width,height) = size.width_height(width, height);
                Some(TextureAlloc{
                    width,
                    height,
                    pixel:TexturePixel::D32,
                    category: TextureCategory::DepthBuffer,
                })
            },
            _=>None
        }
    }

    #[cfg(any(target_os = "android", target_os = "linux"))]
    #[allow(unused)]
    pub(crate) fn as_video_alloc(&self)->Option<TextureAlloc>{
        match self{
            Self::VideoRGB => {
                Some(TextureAlloc{
                    width: 0,
                    height: 0,
                    pixel:TexturePixel::VideoRGB,
                    category: TextureCategory::Video,
                })
            },
            _ => None
        }
    }
    
    #[allow(unused)]
    pub(crate) fn as_shared_alloc(&self)->Option<TextureAlloc>{
        match self{
            Self::SharedBGRAu8{width, height, ..}=>{
                Some(TextureAlloc{
                    width:*width,
                    height:*height,
                    pixel:TexturePixel::BGRAu8,
                    category: TextureCategory::Shared,
                })
            }
            _=>None
        }
    }

    #[allow(unused)]
    fn is_compatible_with(&self, other: &Self) -> bool {
        #[cfg(any(target_os = "android", target_os = "linux"))]
        {
            return !(self.is_video() ^ other.is_video());
        }
        true
    }
}

impl Default for TextureFormat {
    fn default() -> Self {
        TextureFormat::Unknown
    }
}

impl Texture {
    pub fn new(cx: &mut Cx) -> Self {
        cx.null_texture()
    }

    pub fn new_with_format(cx: &mut Cx, format: TextureFormat) -> Self {
        let texture = cx.textures.alloc(format);
        texture
    }
    
    pub fn get_format<'a>(&self, cx: &'a mut Cx) -> &'a mut TextureFormat {
        &mut cx.textures[self.texture_id()].format
    }

    pub fn take_vec_u32(&self, cx: &mut Cx) -> Vec<u32> {
        let cx_texture = &mut cx.textures[self.texture_id()];
        let data = match &mut cx_texture.format {
            TextureFormat::VecBGRAu8_32 { data, .. } => data,
            _ => panic!("incorrect texture format for u32 image data"),
        };
        data.take().expect("image data already taken")
    }

    pub fn put_back_vec_u32(&self, cx: &mut Cx, new_data: Vec<u32>, dirty_rect: Option<RectUsize>) {
        let cx_texture = &mut cx.textures[self.texture_id()];
        let (data, updated) = match &mut cx_texture.format {
            TextureFormat::VecBGRAu8_32 { data, updated, .. } => (data, updated),
            _ => panic!("incorrect texture format for u32 image data"),
        };
        assert!(data.is_none(), "image data not taken or already put back");
        *data = Some(new_data);
        *updated = updated.update(dirty_rect);
    }

    pub fn take_vec_u8(&self, cx: &mut Cx) -> Vec<u8> {
        let cx_texture = &mut cx.textures[self.texture_id()];
        let data = match &mut cx_texture.format {
            TextureFormat::VecRu8 { data, .. } => data,
            TextureFormat::VecRGu8 { data, .. } => data,
            _ => panic!("incorrect texture format for u32 image data"),
        };
        data.take().expect("image data already taken")
    }

    pub fn put_back_vec_u8(&self, cx: &mut Cx, new_data: Vec<u8>, dirty_rect: Option<RectUsize>) {
        let cx_texture = &mut cx.textures[self.texture_id()];
        let (data, updated) = match &mut cx_texture.format {
            TextureFormat::VecRu8 { data, updated, .. } => (data, updated),
            TextureFormat::VecRGu8 { data,updated, .. } => (data, updated),
            _ => panic!("incorrect texture format for u8 image data"),
        };
        assert!(data.is_none(), "image data not taken or already put back");
        *data = Some(new_data);
        *updated = updated.update(dirty_rect);
    }
            
    pub fn take_vec_f32(&self, cx: &mut Cx) -> Vec<f32> {
        let cx_texture = &mut cx.textures[self.texture_id()];
        let data = match &mut cx_texture.format{
            TextureFormat::VecRf32 { data, .. } => data,
            TextureFormat::VecRGBAf32{data, .. } => data,
            _ => panic!("Not the correct texture desc for f32 image data"),
        };
        data.take().expect("image data already taken")
    }

    pub fn put_back_vec_f32(&self, cx: &mut Cx, new_data: Vec<f32>, dirty_rect: Option<RectUsize>) {
        let cx_texture = &mut cx.textures[self.texture_id()];
        let (data, updated) = match &mut cx_texture.format {
            TextureFormat::VecRf32 { data, updated, .. } => (data, updated),
            TextureFormat::VecRGBAf32 { data, updated, .. } => (data, updated),
            _ => panic!("incorrect texture format for f32 image data"),
        };
        assert!(data.is_none(), "image data not taken or already put back");
        *data = Some(new_data);
        *updated = updated.update(dirty_rect);
    }
}

#[derive(Default)]
pub struct CxTexture {
    pub (crate) format: TextureFormat,
    pub (crate) alloc: Option<TextureAlloc>,
    pub os: CxOsTexture,
    pub previous_platform_resource: Option<CxOsTexture>,
}
