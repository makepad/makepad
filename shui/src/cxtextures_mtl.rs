use std::mem;
use metal::*;

#[derive(Default,Clone)]
pub struct Texture{
    pub texture_id: usize,
    pub dirty:bool,
    pub image: Vec<u32>,
    pub width: usize,
    pub height:usize,
    pub mtltexture: Option<metal::Texture>
}

impl Texture{
    pub fn resize(&mut self, width:usize, height:usize){
        self.width = width;
        self.height = height;
        self.image.resize((width * height) as usize, 0);
        self.dirty = true;
    }

    pub fn upload_to_device(&mut self, device:&Device){
        let desc = TextureDescriptor::new();
        desc.set_texture_type(MTLTextureType::D2);
        desc.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
        desc.set_width(self.width as u64);
        desc.set_height(self.height as u64);
        desc.set_storage_mode(MTLStorageMode::Managed);
        //desc.set_mipmap_level_count(1);
        //desc.set_depth(1);
        //desc.set_sample_count(4);
        let tex = device.new_texture(&desc);
    
        let region = MTLRegion{
            origin:MTLOrigin{x:0,y:0,z:0},
            size:MTLSize{width:self.width as u64, height:self.height as u64, depth:1}
        };
        tex.replace_region(region, 0, (self.width * mem::size_of::<u32>()) as u64, self.image.as_ptr() as *const std::ffi::c_void);

        //image_buf.did_modify_range(NSRange::new(0 as u64, (self.image.len() * mem::size_of::<u32>()) as u64));

        self.mtltexture = Some(tex);
        self.dirty = false;
      
    }
}

#[derive(Clone, Default)]
pub struct CxTextures{
    pub textures:Vec<Texture>
}

impl CxTextures{
    pub fn get(&self, id:usize)->&Texture{
        &self.textures[id]
    }

    pub fn add_empty(&mut self)->&mut Texture{
        //let id = self.textures.len();
        let id = self.textures.len();
        self.textures.push(
            Texture{
                texture_id:id,
                ..Default::default()
            }
        );
        &mut self.textures[id]
    }
}
