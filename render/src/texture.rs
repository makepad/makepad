use crate::cx::*;

#[derive(Clone, PartialEq, Debug)]
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

#[derive(Clone, PartialEq, Debug)]
pub struct TextureDesc {
    pub format: TextureFormat,
    pub width: Option<usize>,
    pub height: Option<usize>,
    pub multisample: Option<usize>
}

#[derive(Clone)]
pub struct Texture {
    pub texture_id: Option<usize>,
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

impl Default for Texture {
    fn default() -> Self {
        Texture {
            texture_id: None
        }
    }
}

impl Texture {
    pub fn get_desc(&mut self, cx: &Cx) -> Option<TextureDesc> {
        if let Some(texture_id) = self.texture_id {
            Some(cx.textures[texture_id].desc.clone())
        }
        else {
            None
        }
    }
    
    pub fn set_desc(&mut self, cx: &mut Cx, desc: Option<TextureDesc>) {
        if self.texture_id.is_none() {
            self.texture_id = Some(if cx.textures_free.len() > 0 {
                cx.textures_free.pop().unwrap()
            } else {
                cx.textures.push(CxTexture::default());
                cx.textures.len() - 1
            });
        }
        let cxtexture = &mut cx.textures[self.texture_id.unwrap()];
        if let Some(desc) = desc {
            cxtexture.desc = desc;
        }
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
