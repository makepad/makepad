use {
    std::{
        io::prelude::*,
        fs::File,
        rc::Rc,
        time::{SystemTime}
    },
    crate::{
        cx::{Cx},
    }
};

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum EventFlow{
    Poll,
    Wait,
    Exit
}

// lets start a websocket thread


impl Cx {
    
    pub fn native_load_dependencies(&mut self){
        for (path,dep) in &mut self.dependencies{
            if let Ok(mut file_handle) = File::open(path) {
                let mut buffer = Vec::<u8>::new();
                if file_handle.read_to_end(&mut buffer).is_ok() {
                    dep.data = Some(Ok(Rc::new(buffer)));
                }
                else{
                    dep.data = Some(Err("read_to_end failed".to_string()));
                }
            }
            else{
                println!("Could not load resource {}", path);
                dep.data = Some(Err("File open failed".to_string()));
            }
        }
    }
    
    pub fn time_now()->f64{
        if let Ok(elapsed) = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH){
            return elapsed.as_secs_f64();
        }
        return 0.0
    }
}
