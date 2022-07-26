use {
    std::{
        io::prelude::*,
        fs::File,
        io,
    },
    crate::{
        makepad_math::Vec2,
        event::{
            Event,
            //KeyCode,
        },
        area::Area,
        cx::Cx,
    }
};

#[macro_export]
macro_rules!console_log {
    ( $ ( $t: tt) *) => {
        println!("{}:{} - {}",file!(),line!(),format!($($t)*))
    }
}

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
    
    pub fn get_default_window_size(&self) -> Vec2 {
        return Vec2 {x: 800., y: 600.}
    }
    
    pub(crate) fn process_desktop_pre_event(&mut self, event: &mut Event)
    {
        match event {
            Event::FingerHover(fe) => {
                self.fingers[fe.digit].over_last = Area::Empty;
                //self.hover_mouse_cursor = None;
            },
            Event::FingerUp(_fe) => {
                //self.down_mouse_cursor = None;
            },
            Event::WindowCloseRequested(_cr) => {
            },
            Event::FingerDown(fe) => {
                // lets set the finger tap count
                fe.tap_count = self.process_tap_count(fe.digit, fe.abs, fe.time);
            },
            Event::KeyDown(ke) => {
                self.process_key_down(ke.clone());
            },
            Event::KeyUp(ke) => {
                self.process_key_up(ke.clone());
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
                self.fingers[fe.digit].captured = Area::Empty;
            },
            Event::FingerHover(fe) => { // new last area finger over
                self.fingers[fe.digit]._over_last = self.fingers[fe.digit].over_last;
            },
            Event::FingerScroll(_) => {
            }
            Event::FingerDrag(_) => {
                self.drag_area = self.new_drag_area;
            },
            _ => {}
        }
        false
    }
    
    pub fn write_log(data: &str) {
        let _ = io::stdout().write(data.as_bytes());
        let _ = io::stdout().flush();
    }
}
