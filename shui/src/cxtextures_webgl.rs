use crate::cxshaders::*;

#[derive(Default,Clone)]
pub struct Texture{
    pub texture_id: usize,
    pub dirty:bool,
    pub image: Vec<u32>,
    pub width: usize,
    pub height:usize
}

impl Texture{
    pub fn resize(&mut self, width:usize, height:usize){
        self.width = width;
        self.height = height;
        self.image.resize((width * height) as usize, 0);
        self.dirty = true;
    }

    pub fn upload_to_device(&mut self, resources:&mut CxResources){
        resources.from_wasm.alloc_texture(self.texture_id, self.width, self.height, &self.image);
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