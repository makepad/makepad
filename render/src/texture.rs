use crate::cx::*;


#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Texture {
    pub texture_id: u32,
}

#[derive(Copy, Clone, Default, PartialEq, Debug)]
pub struct Texture2D(pub Option<u32>);

impl Into<Texture2D> for Texture {
    fn into(self) -> Texture2D {
        Texture2D(Some(self.texture_id as u32))
    }
}

#[derive(Clone, PartialEq)]
pub enum TextureFormat {
    Default,
    ImageBGRA,
    Depth32Stencil8,
    RenderBGRA,
    RenderBGRAf16,
    RenderBGRAf32,
    //    ImageBGRAf32,
    //    ImageRf32,
    //    ImageRGf32,
    //    MappedBGRA,
    //    MappedBGRAf32,
    //    MappedRf32,
    //    MappedRGf32,
}

#[derive(Clone, PartialEq)]
pub struct TextureDesc {
    pub format: TextureFormat,
    pub width: Option<usize>,
    pub height: Option<usize>,
    pub multisample: Option<usize>
}

impl Default for TextureDesc {
    fn default() -> Self {
        TextureDesc {
            format: TextureFormat::Default,
            width: None,
            height: None,
            multisample: None
        }
    }
}



pub trait TextureCx {
    fn new(cx:&mut Cx)->Texture;
    fn set_desc(&mut self, cx:&mut Cx, desc:TextureDesc);
    fn get_desc(&self, cx:&mut Cx) -> TextureDesc;
}


impl TextureCx for Texture{
    fn new(cx:&mut Cx)->Texture{
        Texture{
            texture_id:if cx.textures_free.len() > 0 {
                cx.textures_free.pop().unwrap() as u32
            } else {
                cx.textures.push(CxTexture::default());
                (cx.textures.len() - 1) as u32
            }
        }
    }

    fn set_desc(&mut self, cx:&mut Cx, desc:TextureDesc){
        let cxtexture = &mut cx.textures[self.texture_id as usize];
        cxtexture.desc = desc;
    }

    fn get_desc(&self, cx:&mut Cx) -> TextureDesc {
        cx.textures[self.texture_id as usize].desc.clone()
    }
}


#[derive(Default)]
pub struct CxTexture {
    pub desc: TextureDesc,
    pub image_u32: Vec<u32>,
    pub image_f32: Vec<f32>,
    pub update_image: bool,
    pub platform: CxPlatformTexture
}
