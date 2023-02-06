use {
    std::{
        io::prelude::*,
        fs::File,
    },
    crate::{
        cx::Cx,
    }
};

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum EventFlow{
    Poll,
    Wait,
    Exit
}

impl Cx {
    
    pub fn native_load_dependencies(&mut self){
        for (path,dep) in &mut self.dependencies{
            if let Ok(mut file_handle) = File::open(path) {
                let mut buffer = Vec::<u8>::new();
                if file_handle.read_to_end(&mut buffer).is_ok() {
                    dep.data = Some(Ok(buffer));
                }
                else{
                    dep.data = Some(Err("read_to_end failed".to_string()));
                }
            }
            else{
                dep.data = Some(Err("File open failed".to_string()));
            }
        }
    }
}
