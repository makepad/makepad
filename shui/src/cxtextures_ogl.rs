use std::mem;

#[derive(Default,Clone)]
pub struct Texture{
    pub texture_id: usize,
    pub dirty:bool,
    pub image: Vec<u32>,
    pub width: usize,
    pub height:usize,
    pub gl_texture: Option<gl::types::GLuint>
}

impl Texture{
    pub fn resize(&mut self, width:usize, height:usize){
        self.width = width;
        self.height = height;
        self.image.resize((width * height) as usize, 0);
        self.dirty = true;
    }

    pub fn upload_to_device(&mut self){

        unsafe{
            let mut tex_handle;
            match self.gl_texture{
                None=>{
                    tex_handle = mem::uninitialized();
                    gl::GenTextures(1, &mut tex_handle);
                    self.gl_texture = Some(tex_handle);
                }
                Some(gl_texture)=>{
                    tex_handle = gl_texture
                }
            }
            gl::BindTexture(gl::TEXTURE_2D, tex_handle);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::LINEAR as i32);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::LINEAR as i32);
            gl::TexImage2D(gl::TEXTURE_2D, 0, gl::RGBA as i32, self.width as i32, self.height as i32, 0, gl::RGBA, gl::UNSIGNED_BYTE, self.image.as_ptr() as *const _);
            gl::BindTexture(gl::TEXTURE_2D, 0);
        }

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