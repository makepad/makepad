pub use {
    std::{
        rc::Rc,
        cell::RefCell
    },
    crate::{
        makepad_live_compiler::*,
        cx::Cx,
        platform::{CxPlatformTexture},
        live_traits::*
    }
};


#[derive(PartialEq)]
pub struct Texture {
    pub texture_id: usize,
    pub textures_free: Rc<RefCell<Vec<usize>>>,
}

#[derive(Clone, Copy, PartialEq)]
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



pub trait TextureCx {
    fn set_desc(&mut self, cx:&mut Cx, desc:TextureDesc);
    fn get_desc(&self, cx:&mut Cx) -> TextureDesc;
}


impl LiveHook for Texture{}
impl LiveNew for Texture {
    fn new(cx: &mut Cx)->Self{
        let textures_free = cx.textures_free.clone();
        let texture_id =  if let Some(texture_id) = textures_free.borrow_mut().pop(  ){
            texture_id 
        }
        else{
            let texture_id = cx.textures.len();
            cx.textures.push(CxTexture::default());
            texture_id
        };        
        Self{
            texture_id,
            textures_free
        }
    }
    
    fn live_type_info(_cx:&mut Cx) -> LiveTypeInfo{
        LiveTypeInfo {
            module_id: LiveModuleId::from_str(&module_path!()).unwrap(),
            live_type: LiveType::of::<Self>(),
            fields: Vec::new(),
            //kind: LiveTypeKind::Object,
            type_name: LiveId::from_str("Texture").unwrap()
        }
    }
}

impl LiveApply for Texture {
    //fn type_id(&self)->std::any::TypeId{ std::any::TypeId::of::<Self>()}
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


impl TextureCx for Texture{
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
