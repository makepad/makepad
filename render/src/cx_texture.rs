use crate::cx::*;

#[derive(Clone, PartialEq, Debug)]
pub enum TextureUsage {
    Default,
    Image2D,
    RenderTarget2D,
    DepthBuffer,
}

#[derive(Clone, PartialEq, Debug)]
pub enum TexturePixel {
    Default,
    BGRA8Unorm,
    BGRAFloat16,
    BGRAFloat32,
    Depth24UnormStencil8
}

#[derive(Clone, PartialEq, Debug)]
pub struct TextureDesc {
    pub pixel: TexturePixel,
    pub usage: TextureUsage,
    pub width: Option<usize>,
    pub height: Option<usize>,
    pub samples: usize
}

#[derive(Clone)]
pub struct Texture {
    pub texture_id: Option<usize>,
}

impl Default for TextureDesc {
    fn default() -> Self {
        TextureDesc {
            pixel: TexturePixel::Default,
            usage: TextureUsage::Default,
            width: None,
            height: None,
            samples: 1
        }
    }
}

impl Default for Texture {
    fn default() -> Self {
        Texture {
            texture_id: None
        }
    }
}

impl Texture{
    pub fn get_desc(&mut self, cx:&Cx)->Option<TextureDesc>{
        if let Some(texture_id) = self.texture_id{
            Some(cx.textures[texture_id].desc.clone())
        }
        else{
            None
        }
    }
    
    pub fn set_desc(&mut self, cx:&mut Cx, desc:Option<TextureDesc>){
        if self.texture_id.is_none(){
            if cx.textures_free.len() > 0{
                self.texture_id = Some(cx.textures_free.pop().unwrap())
            }
            else{
                self.texture_id = Some(cx.textures.len());
                cx.textures.push(CxTexture{..Default::default()});
            };
        }
        let cxtexture = &mut cx.textures[self.texture_id.unwrap()];
        if let Some(desc) = desc{
            cxtexture.desc = desc;
        }
    }
}

#[derive(Default, Clone)]
pub struct CxTexture{
    pub desc:TextureDesc,
    pub buffer_u32: Vec<u32>,
    pub buffer_f32: Vec<f32>,
    pub upload_buffer: bool,
    pub platform: CxPlatformTexture
}
