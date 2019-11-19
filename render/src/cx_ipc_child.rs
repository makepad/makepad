
use crate::cx::*;

impl Cx {
    
    pub fn get_default_window_size(&self) -> Vec2 {
        return self.platform.window_geom.inner_size;
    }

    pub fn process_to_wasm<F>(&mut self, _msg: u32, mut _event_handler: F) -> u32
    where F: FnMut(&mut Cx, &mut Event){
        0
    }
    
    // empty stub
    pub fn event_loop<F>(&mut self, mut _event_handler: F)
    where F: FnMut(&mut Cx, Event){
        // ok so. now what.
    }
    
    pub fn write_log(&mut self, _data: &str) {
    }
    
    pub fn post_signal(_signal: Signal, _value: usize) {
    }
    
    pub fn file_read(&mut self, path: &str) -> FileRead {
        FileRead {read_id: 0, path: path.to_string()}
    }
    
    pub fn file_write(&mut self, _path: &str, _data: &[u8]) -> u64 {
        return 0
    }
    
    pub fn set_window_outer_size(&mut self, _size: Vec2) {
    }
    
    pub fn set_window_position(&mut self, _pos: Vec2) {
    }
    
    pub fn show_text_ime(&mut self, _x: f32, _y: f32) {
    }
    
    pub fn hide_text_ime(&mut self) {
    }
    
    pub fn start_timer(&mut self, _interval: f64, _repeats: bool) -> Timer {
        self.timer_id += 1;
        Timer {timer_id: 0}
    }
    
    pub fn stop_timer(&mut self, timer: &mut Timer) {
        if timer.timer_id != 0 {
            timer.timer_id = 0;
        }
    }
    
    pub fn http_send(&self, _verb: &str, _path: &str, _domain: &str, _port: &str, _body: &str) {
    }
}


// storage buffers for graphics API related platform
#[derive(Clone)]
pub struct CxPlatform {
    pub window_geom: WindowGeom,
    pub fingers_down: Vec<bool>,
    pub file_read_id: u64,
}

impl Default for CxPlatform {
    fn default() -> CxPlatform {
        CxPlatform {
            window_geom: WindowGeom::default(),
            file_read_id: 1,
            fingers_down: Vec::new()
        }
    }
}

#[derive(Clone, Default)]
pub struct CxPlatformDrawCall {
}


#[derive(Clone, Default)]
pub struct CxPlatformShader {
}

#[derive(Clone, Default)]
pub struct CxPlatformView {
}

#[derive(Default, Clone, Debug)]
pub struct CxPlatformTexture {
}

#[derive(Default, Clone, Debug)]
pub struct CxPlatformPass {
}


impl<'a> SlCx<'a> {
    pub fn map_call(&self, _name: &str, _args: &Vec<Sl>) -> MapCallResult {
        return MapCallResult::None
    }
    
    pub fn mat_mul(&self, left: &str, right: &str) -> String {
        format!("{}*{}", left, right)
    }
    
    pub fn map_type(&self, ty: &str) -> String {
        ty.to_string()
    }
    
    pub fn map_var(&mut self, var: &ShVar) -> String {
        var.name.clone()
    }
}