use {
    crate::{
        id_pool::*,
        makepad_error_log::*,
        cx::Cx,
        os::CxOsTexture,
    },
    std::rc::Rc,
};


#[derive(Clone)]
pub struct Texture(Rc<PoolId>);

#[derive(Clone, Debug, PartialEq, Copy)]
pub struct TextureId(pub (crate) usize, u64);

impl Texture {
    pub fn texture_id(&self) -> TextureId {TextureId(self.0.id, self.0.generation)}
}

#[derive(Default)]
pub struct CxTexturePool(pub (crate) IdPool<CxTexture>);
impl CxTexturePool {
    pub fn alloc(&mut self) -> Texture {
        Texture(Rc::new(self.0.alloc()))
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
    VecBGRAu8{width:usize, height:usize, data:Vec<u32>},
    VecMipBGRAu8{width:usize, height:usize, data:Vec<u32>, max_level:Option<usize>},
    VecBGRAf32{width:usize, height:usize, data:Vec<f32>},
    VecRu8{width:usize, height:usize, data:Vec<u8>},
    VecRf32{width:usize, height:usize, data:Vec<f32>},
    DepthD32S8{size:TextureSize},
    RenderBGRAu8{size:TextureSize},
    RenderBGRAf16{size:TextureSize},
    RenderBGRAf32{size:TextureSize},
    SharedBGRAu8{width:usize, height:usize, id:crate::cx_stdin::PresentableImageId},
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TextureAlloc{
    pub category: TextureCategory,
    pub pixel: TexturePixel,
    pub width: usize,
    pub height: usize,
}

#[derive(Clone, Debug)]
pub(crate) enum TextureCategory{
    Vec{updated:bool},
    Render{initial:bool},
    DepthBuffer{initial:bool},
    Shared{initial:bool},
}

impl PartialEq for TextureCategory{
    fn eq(&self, other: &TextureCategory) -> bool{
        match self{
            Self::Vec{..} => if let Self::Vec{..} = other{true} else {false},
            Self::Render{..} => if let Self::Render{..} = other{true} else {false},
            Self::Shared{..} => if let Self::Shared{..} = other{true} else {false},
            Self::DepthBuffer{..} => if let Self::DepthBuffer{..} = other{true} else {false},           
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TexturePixel{
    BGRAu8,
    BGRAf16,
    BGRAf32,
    Ru8,
    Rf32,
    D32S8,
}

impl CxTexture{
    pub(crate) fn set_updated(&mut self, up:bool){
        if let Some(alloc) = &mut self.alloc{
            if let TextureCategory::Vec{updated} = &mut alloc.category{
                *updated = up
            }
        }
    }
    
    pub(crate) fn check_updated(&mut self)->bool{
        if let Some(alloc) = &mut self.alloc{
            if let TextureCategory::Vec{updated} = &mut alloc.category{
                let u = *updated;
                *updated = false;
                return u
            }
        }
        false
    }
    
    pub(crate) fn _set_initial(&mut self, init:bool){
        if let Some(alloc) = &mut self.alloc{
            match &mut alloc.category{
                TextureCategory::Render{initial} |
                TextureCategory::DepthBuffer{initial} |
                TextureCategory::Shared{initial}=>{
                    *initial = init;
                }
                _=>()
            }
        }
    }
        
    pub(crate) fn check_initial(&mut self)->bool{
        if let Some(alloc) = &mut  self.alloc{
            match &mut alloc.category{
                TextureCategory::Render{initial} |
                TextureCategory::DepthBuffer{initial} |
                TextureCategory::Shared{initial}=>{
                    let u = *initial;
                    *initial = false;
                    return u
                }
                _=>()
            }
        }
        false
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
            Self::VecBGRAu8{..}=>true,
            Self::VecBGRAf32{..}=>true,
            Self::VecRu8{..}=>true,
            Self::VecRf32{..}=>true,
            _=>false
        }
    }
    
    pub fn is_render(&self)->bool{
        match self{
            Self::RenderBGRAu8{..}=>true,
            Self::RenderBGRAf16{..}=>true,
            Self::RenderBGRAf32{..}=>true,
            _=>false
        }
    }
        
    pub fn is_depth(&self)->bool{
        match self{
            Self::DepthD32S8{..}=>true,
            _=>false
        }
    }
    
    pub fn vec_width_height(&self)->Option<(usize,usize)>{
        match self{
            Self::VecBGRAu8{width, height, .. }=>Some((*width,*height)),
            Self::VecMipBGRAu8{width, height, ..}=>Some((*width,*height)),
            Self::VecBGRAf32{width, height, ..}=>Some((*width,*height)),
            Self::VecRu8{width, height, ..}=>Some((*width,*height)),
            Self::VecRf32{width, height,..}=>Some((*width,*height)),
            _=>None
        }
    }
    
    pub(crate) fn as_vec_alloc(&self)->Option<TextureAlloc>{
        match self{
            Self::VecBGRAu8{width,height,..}=>Some(TextureAlloc{
                width:*width,
                height:*height,
                pixel:TexturePixel::BGRAu8,
                category: TextureCategory::Vec{updated:true}
            }),
            Self::VecBGRAf32{width,height,..}=>Some(TextureAlloc{
                width:*width,
                height:*height,
                pixel:TexturePixel::BGRAf32,
                category: TextureCategory::Vec{updated:true}
            }),
            Self::VecRu8{width,height,..}=>Some(TextureAlloc{
                width:*width,
                height:*height,
                pixel:TexturePixel::Ru8,
                category: TextureCategory::Vec{updated:true}
            }),
            Self::VecRf32{width,height,..}=>Some(TextureAlloc{
                width:*width,
                height:*height,
                pixel:TexturePixel::Rf32,
                category: TextureCategory::Vec{updated:true}
            }),
            _=>None
        }
    }
        
    pub(crate) fn as_render_alloc(&self, width:usize, height:usize)->Option<TextureAlloc>{
        match self{
            Self::RenderBGRAu8{size,..}=>{
                let (width,height) = size.width_height(width, height);
                Some(TextureAlloc{
                    width,
                    height,
                    pixel:TexturePixel::BGRAu8,
                    category: TextureCategory::Render{initial:true}
                })
            }
            Self::RenderBGRAf16{size,..}=>{
                let (width,height) = size.width_height(width, height);
                Some(TextureAlloc{
                    width,
                    height,
                    pixel:TexturePixel::BGRAf16,
                    category: TextureCategory::Render{initial:true}
                })
            }
            Self::RenderBGRAf32{size,..}=>{
                let (width,height) = size.width_height(width, height);
                Some(TextureAlloc{
                    width,
                    height,
                    pixel:TexturePixel::BGRAf32,
                    category: TextureCategory::Render{initial:true}
                })
            }
            _=>None
        }
    }
        
    pub(crate) fn as_depth_alloc(&self, width:usize, height:usize)->Option<TextureAlloc>{
        match self{
            Self::DepthD32S8{size,..}=>{
                let (width,height) = size.width_height(width, height);
                Some(TextureAlloc{
                    width,
                    height,
                    pixel:TexturePixel::D32S8,
                    category: TextureCategory::DepthBuffer{initial:true}
                })
            },
            _=>None
        }
    }
    
    pub(crate) fn as_shared_alloc(&self)->Option<TextureAlloc>{
        match self{
            Self::SharedBGRAu8{width, height, ..}=>{
                Some(TextureAlloc{
                    width:*width,
                    height:*height,
                    pixel:TexturePixel::BGRAu8,
                    category: TextureCategory::Shared{initial:true},
                })
            }
            _=>None
        }
    }
}

impl Default for TextureFormat {
    fn default() -> Self {
        TextureFormat::Unknown
    }
}

impl Texture {
    pub fn new(cx: &mut Cx) -> Self {
        let texture = cx.textures.alloc();
        texture
    }
    
    pub fn set_format(&self, cx: &mut Cx, format: TextureFormat) {
        let cxtexture = &mut cx.textures[self.texture_id()];
        cxtexture.format = format;
    }
    
    pub fn get_format<'a>(&self, cx: &'a mut Cx) -> &'a mut TextureFormat {
        &mut cx.textures[self.texture_id()].format
    }
    
    pub fn swap_vec_u32(&self, cx: &mut Cx, image: &mut Vec<u32>) {
        let cxtexture = &mut cx.textures[self.texture_id()];
        match &mut cxtexture.format{
            TextureFormat::VecBGRAu8{data,..} => {
                std::mem::swap(data, image);
                cxtexture.set_updated(true);
            }
            _=>{
                panic!("Not the correct texture desc for u32 image buffer")
            }
        }
    }
            
    pub fn swap_vec_u8(&self, cx: &mut Cx, image: &mut Vec<u8>) {
        let cxtexture = &mut cx.textures[self.texture_id()];
        match &mut cxtexture.format{
            TextureFormat::VecRu8{data,..} => {
                std::mem::swap(data, image);
                cxtexture.set_updated(true);
            }
            _=>{
                panic!("Not the correct texture desc for u8 image buffer")
            }
        }
    }
            
    pub fn swap_vec_f32(&self, cx: &mut Cx, image: &mut Vec<f32>) {
        let cxtexture = &mut cx.textures[self.texture_id()];
        match &mut cxtexture.format{
            TextureFormat::VecRf32{data,..} => {
                std::mem::swap(data, image);
                cxtexture.set_updated(true);
            }
            _=>{
                panic!("Not the correct texture desc for f32 image buffer")
            }
        }
    }
}

#[derive(Default)]
pub struct CxTexture {
    pub (crate) format: TextureFormat,
    pub (crate) alloc: Option<TextureAlloc>,
    pub os: CxOsTexture
}
