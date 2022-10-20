use {
    std::{
        io::prelude::*,
        fs::File,
    },
    crate::{
        cx::Cx,
    }
};

impl Cx {
    
    pub fn desktop_load_dependencies(&mut self){
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
    
    /*
    pub(crate) fn process_desktop_pre_event(&mut self, event: &mut Event)
    {
        match event {
            Event::FingerDown(fe) => {
                fe.tap_count = self.fingers.process_tap_count(fe.digit_id, fe.abs, fe.time);
            },
            Event::KeyDown(ke) => {
                self.keyboard.process_key_down(ke.clone());
            },
            Event::KeyUp(ke) => {
                self.keyboard.process_key_up(ke.clone());
            },
            Event::AppLostFocus => {
                self.call_all_keys_up();
            },
            _ => ()
        };
    }
    
    pub(crate) fn process_desktop_post_event(&mut self, event: &mut Event) -> bool {
        match event {
            Event::FingerUp(fe) => { // decapture automatically
                self.fingers.release_digit(fe.digit);
            },
            Event::FingerHover(_) => { // new last area finger over
                self.fingers.cycle_over_last();
            },
            Event::FingerDrag(_) => {
                self.finger_drag.cycle_drag();
            },
            _ => {}
        }
        false
    }*/
}
