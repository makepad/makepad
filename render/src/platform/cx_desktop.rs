use {
    std::{
        io::prelude::*,
   //     fs::File,
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
macro_rules!log {
    ( $ ( $t: tt) *) => {
        println!("{}:{} - {}",file!(),line!(),format!($($t)*))
    }
}

#[derive(Clone)]
pub struct CxDesktop {
    pub repaint_via_scroll_event: bool,
    //pub file_read_id: u64,
    //pub file_reads: Vec<FileRead>,
    pub profiler_start: Option<u64>,
}

impl Default for CxDesktop {
    fn default() -> CxDesktop {
        CxDesktop {
            repaint_via_scroll_event: false,
            //file_read_id: 1,
            //file_reads: Vec::new(),
            profiler_start: None,
        }
    }
}

impl Cx {
    
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
                self.down_mouse_cursor = None;
            },
            Event::WindowCloseRequested(_cr) => {
            },
            Event::FingerDown(fe) => {
                // lets set the finger tap count
                fe.tap_count = self.process_tap_count(fe.digit, fe.abs, fe.time);
            },
            Event::KeyDown(ke) => {
                self.process_key_down(ke.clone());
                /*if ke.key_code == KeyCode::PrintScreen {
                    if ke.modifiers.control {
                        self.panic_redraw = true;
                    }
                    else {
                        self.panic_now = true;
                    }
                }*/
            },
            Event::KeyUp(ke) => {
                self.process_key_up(&ke);
            },
            Event::AppFocusLost => {
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
                //if fe.hover_state == HoverState::Out{
                //    self.hover_mouse_cursor = None;
                //}
            },
            Event::FingerScroll(_) => {
                // check for anything being paint or dra dirty
                if self.need_redrawing() {
                    self.platform.desktop.repaint_via_scroll_event = true;
                }
            }
            Event::FingerDrag(_) => {
                self.drag_area = self.new_drag_area;
            },
            _ => {}
        }
        false
    }
    
    pub(crate) fn process_desktop_paint_callbacks(&mut self, time: f64) -> bool
    {
        let mut vsync = false; //self.platform.desktop.repaint_via_scroll_event;
        self.platform.desktop.repaint_via_scroll_event = false;
        if self.new_next_frames.len() != 0 {
            self.call_next_frame_event(time);
            if self.new_next_frames.len() != 0 {
                vsync = true;
            }
        }
        
        self.call_signals_and_triggers();
        
        // call redraw event
        if self.need_redrawing(){
            self.call_draw_event();
        }

        if self.need_redrawing(){
            vsync = true;
        }
        
        self.call_signals_and_triggers();
        
        vsync
    }
    
    pub(crate) fn process_to_wasm<F>(&mut self, _msg: u32, mut _event_handler: F) -> u32
    where F: FnMut(&mut Cx, &mut Event)
    {
        0
    }
    
    pub fn write_log(data: &str) {
        let _ = io::stdout().write(data.as_bytes());
        let _ = io::stdout().flush();
    }
}
