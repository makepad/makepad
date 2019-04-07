use render::*;

#[derive(Clone, Default)]
pub struct TextBuffer{
    pub text: String,
    pub load_id: u64
}

impl TextBuffer{

    pub fn save_buffer(&mut self){

    }

    pub fn load_buffer(&mut self, data:&Vec<u8>){

    }
}