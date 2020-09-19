use crate::cx::*;

pub trait TextureCx {
    fn new(cx:&mut Cx)->Texture;
    fn set_desc(&mut self, cx:&mut Cx, desc:TextureDesc);
    fn get_desc(&self, cx:&mut Cx) -> TextureDesc;
}

impl TextureCx for Texture{
    fn new(cx:&mut Cx)->Texture{
        Texture{
            texture_id:if cx.textures_free.len() > 0 {
                cx.textures_free.pop().unwrap()
            } else {
                cx.textures.push(CxTexture::default());
                cx.textures.len() - 1
            }
        }
    }

    fn set_desc(&mut self, cx:&mut Cx, desc:TextureDesc){
        let cxtexture = &mut cx.textures[self.texture_id];
        cxtexture.desc = desc;
    }

    fn get_desc(&self, cx:&mut Cx) -> TextureDesc {
        cx.textures[self.texture_id].desc.clone()
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
