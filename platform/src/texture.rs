pub use {
    std::{
        sync::Arc,
        rc::Rc,
        cell::RefCell
    },
    crate::{
        id_pool::*,
        makepad_live_compiler::*,
        makepad_live_id::*,
        cx::Cx,
        platform::{CxPlatformTexture},
        live_traits::*
    }
};


pub struct Texture(PoolId);

#[derive(Clone, Debug, PartialEq, Copy)]
pub struct TextureId(usize);

impl Texture{
    pub fn texture_id(&self)->TextureId{TextureId(self.0.id)}
}

#[derive(Default)]
pub struct CxTexturePool(IdPool<CxTexture>);
impl CxTexturePool{
    pub fn alloc(&mut self)->Texture{
        Texture(self.0.alloc())
    }
    /*pub fn alloc_new(&mut self, t:CxTexture)->Texture{
        Texture(self.0.alloc_new(t))
    }*/
}

impl std::ops::Index<TextureId> for CxTexturePool{
    type Output = CxTexture;
    fn index(&self, index: TextureId) -> &Self::Output{
        &self.0.pool[index.0].item
    }
}

impl std::ops::IndexMut<TextureId> for CxTexturePool{
    fn index_mut(&mut self, index: TextureId) -> &mut Self::Output{
        &mut self.0.pool[index.0].item
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
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

#[derive(Clone, Copy, PartialEq)]
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

impl LiveHook for Texture{}
impl LiveNew for Texture {
    fn new(cx: &mut Cx)->Self{
        let texture = cx.textures.alloc();
        texture
    }
    
    fn live_type_info(_cx:&mut Cx) -> LiveTypeInfo{
        LiveTypeInfo {
            module_id: LiveModuleId::from_str(&module_path!()).unwrap(),
            live_type: LiveType::of::<Self>(),
            live_ignore: true,
            fields: Vec::new(),
            type_name: LiveId::from_str("Texture").unwrap()
        }
    }
}

impl LiveApply for Texture {
    fn apply(&mut self, cx: &mut Cx, _apply_from: ApplyFrom, start_index: usize, nodes: &[LiveNode]) -> usize {
        
        if !nodes[start_index].value.is_structy_type() {
            cx.apply_error_wrong_type_for_struct(live_error_origin!(), start_index, nodes, id!(View));
            return nodes.skip_node(start_index);
        }
        
        let mut index = start_index + 1;
        loop {
            if nodes[index].value.is_close() {
                index += 1;
                break;
            }
            match nodes[index].id {
                _=> {
                    cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
                    index = nodes.skip_node(index);
                }
            }
        }
        return index;
    }
}


impl Texture{
    pub fn set_desc(&self, cx:&mut Cx, desc:TextureDesc){
        let cxtexture = &mut cx.textures[self.texture_id()];
        cxtexture.desc = desc;
    }

    pub fn get_desc(&self, cx:&mut Cx) -> TextureDesc {
        cx.textures[self.texture_id()].desc.clone()
    }
    
    pub fn swap_image_u32(&self, cx: &mut Cx, image_u32:&mut Vec<u32>){
        let cxtexture = &mut cx.textures[self.texture_id()];
        std::mem::swap(&mut cxtexture.image_u32, image_u32);
        cxtexture.update_image = true;
    }
}


#[derive(Default)]
pub struct CxTexture {
    pub(crate) desc: TextureDesc,
    pub(crate) image_u32: Vec<u32>,
    //pub(crate) _image_f32: Vec<f32>,
    pub(crate) update_image: bool,
    pub platform: CxPlatformTexture
}
