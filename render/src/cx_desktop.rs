use crate::cx::*;
use std::io::prelude::*;
use std::fs::File;
use std::io;
use std::net::TcpStream;
use time::precise_time_ns;

#[derive(Clone)]
pub struct CxDesktop {
    pub file_read_id: u64,
    pub file_reads: Vec<FileRead>,
    pub profiler_start: Option<u64>,
}

impl Default for CxDesktop {
    fn default() -> CxDesktop {
        CxDesktop {
            file_read_id: 1,
            file_reads: Vec::new(),
            profiler_start: None,
        }
    }
}

impl Cx {
    
    pub fn get_default_window_size(&self) -> Vec2 {
        return Vec2 {x: 800., y: 600.}
    }
    
    pub fn file_read(&mut self, path: &str) -> FileRead {
        let desktop = &mut self.platform.desktop;
        desktop.file_read_id += 1;
        let read_id = desktop.file_read_id;
        let file_read = FileRead {
            read_id: read_id,
            path: path.to_string()
        };
        desktop.file_reads.push(file_read.clone());
        file_read
    }
    
    pub fn file_write(&mut self, path: &str, data: &[u8]) -> u64 {
        // just write it right now
        if let Ok(mut file) = File::create(path) {
            if let Ok(_) = file.write_all(&data) {
            }
            else {
                println!("ERROR WRITING FILE {}", path);
            }
        }
        else {
            println!("ERROR WRITING FILE {}", path);
        }
        0
    }
    
    pub fn process_desktop_pre_event<F>(&mut self, event: &mut Event, mut event_handler: F)
    where F: FnMut(&mut Cx, &mut Event)
    {
        match event {
            Event::FingerHover(_fe) => {
                self.finger_over_last_area = Area::Empty;
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
                if ke.key_code == KeyCode::PrintScreen {
                    if ke.modifiers.control {
                        self.panic_redraw = true;
                    }
                    else {
                        self.panic_now = true;
                    }
                }
            },
            Event::KeyUp(ke) => {
                self.process_key_up(&ke);
            },
            Event::AppFocusLost => {
                self.call_all_keys_up(&mut event_handler);
            },
            _ => ()
        };
    }
    
    pub fn process_desktop_post_event(&mut self, event: &mut Event) -> bool {
        match event {
            Event::FingerUp(fe) => { // decapture automatically
                self.captured_fingers[fe.digit] = Area::Empty;
            },
            Event::FingerHover(_fe) => { // new last area finger over
                self._finger_over_last_area = self.finger_over_last_area;
                //if fe.hover_state == HoverState::Out{
                //    self.hover_mouse_cursor = None;
                //}
            },
            _ => {}
        }
        false
    }
    
    pub fn process_desktop_paint_callbacks<F>(&mut self, time: f64, mut event_handler: F) -> bool
    where F: FnMut(&mut Cx, &mut Event)
    {
        if self.playing_anim_areas.len() != 0 {
            self.call_animation_event(&mut event_handler, time);
        }
        
        let mut vsync = false;
        
        if self.frame_callbacks.len() != 0 {
            self.call_frame_event(&mut event_handler, time);
            if self.frame_callbacks.len() != 0 {
                vsync = true;
            }
        }
        
        self.call_signals(&mut event_handler);
        
        // call redraw event
        if self.redraw_child_areas.len()>0 || self.redraw_parent_areas.len()>0 {
            self.call_draw_event(&mut event_handler);
        }
        if self.redraw_child_areas.len()>0 || self.redraw_parent_areas.len()>0 {
            vsync = true;
        }
        
        self.process_desktop_file_reads(&mut event_handler);
        
        self.call_signals(&mut event_handler);
        
        vsync
    }
    
    
    pub fn process_desktop_file_reads<F>(&mut self, mut event_handler: F)
    where F: FnMut(&mut Cx, &mut Event)
    {
        if self.platform.desktop.file_reads.len() == 0 {
            return
        }
        
        let file_read_requests = self.platform.desktop.file_reads.clone();
        self.platform.desktop.file_reads.truncate(0);
        
        for read_req in file_read_requests {
            let file_result = File::open(&read_req.path);
            if let Ok(mut file) = file_result {
                let mut buffer = Vec::new();
                // read the whole file
                if file.read_to_end(&mut buffer).is_ok() {
                    event_handler(self, &mut Event::FileRead(FileReadEvent {
                        read_id: read_req.read_id,
                        data: Ok(buffer)
                    }))
                }
                else {
                    event_handler(self, &mut Event::FileRead(FileReadEvent {
                        read_id: read_req.read_id,
                        data: Err(format!("Failed to read {}", read_req.path))
                    }))
                }
            }
            else {
                event_handler(self, &mut Event::FileRead(FileReadEvent {
                    read_id: read_req.read_id,
                    data: Err(format!("Failed to open {}", read_req.path))
                }))
            }
        }
        
        if self.platform.desktop.file_reads.len() != 0 {
            self.process_desktop_file_reads(event_handler);
        }
    }
    
    pub fn process_to_wasm<F>(&mut self, _msg: u32, mut _event_handler: F) -> u32
    where F: FnMut(&mut Cx, &mut Event)
    {
        0
    }
    
    pub fn load_theme_fonts(&mut self) {
        // lets load all fonts that aren't loaded yet
        for cxfont in &mut self.fonts{
            let path = cxfont.path.clone();
            if cxfont.font_loaded.is_none() {
                // load it
                let file_result = File::open(&path);
                if let Ok(mut file) = file_result {
                    let mut buffer = Vec::<u8>::new();
                    // read the whole file
                    if file.read_to_end(&mut buffer).is_ok() {
                        let mut font = CxFont::default();
                        if font.load_from_ttf_bytes(&buffer).is_err() {
                            println!("Error loading font {} ", path);
                        }
                        else {
                            font.path = path.clone();
                            *cxfont = font;
                        }
                    }
                }
                else {
                    println!("Error loading font {} ", path);
                }
                
            }
        }
    }
    
    /*pub fn log(&mut self, val:&str){
        let mut stdout = io::stdout();
        let _e = stdout.write(val.as_bytes());
        let _e = stdout.flush();
    }*/
    
    pub fn write_log(data: &str) {
        let _ = io::stdout().write(data.as_bytes());
        let _ = io::stdout().flush();
    }
    
    pub fn http_send(&self, verb: &str, path: &str, domain: &str, port: &str, body: &str) {
        let host = format!("{}:{}", domain, port);
        let stream = TcpStream::connect(&host);
        if let Ok(mut stream) = stream {
            let byte_len = body.as_bytes().len();
            let data = format!("{} /{} HTTP/1.1\r\nHost: {}\r\nConnect: close\r\nContent-Length:{}\r\n\r\n{}", verb, path, domain, byte_len, body);
            if let Err(e) = stream.write(data.as_bytes()) {
                println!("http_send error writing stream {}", e);
            }
        }
        else {
            println!("http_send error connecting");
        }
    }
    
    
    pub fn profile(&mut self) {
        if let Some(start) = self.platform.desktop.profiler_start {
            let delta = precise_time_ns() - start;
            println!("Profile time:{} usec", delta / 1_000);
            self.platform.desktop.profiler_start = None
        }
        else {
            self.platform.desktop.profiler_start = Some(precise_time_ns())
        }
    }
}