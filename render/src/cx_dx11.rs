use crate::cx_windows::*;
use crate::cx::*;
use crate::cx_desktop::*;

impl Cx {
     
    pub fn exec_draw_list(&mut self, draw_list_id: usize) {
        
        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
        let draw_calls_len = self.draw_lists[draw_list_id].draw_calls_len;
        for draw_call_id in 0..draw_calls_len {
            let sub_list_id = self.draw_lists[draw_list_id].draw_calls[draw_call_id].sub_list_id;
            if sub_list_id != 0 {
                self.exec_draw_list(sub_list_id);
            }
            else {
                let draw_list = &mut self.draw_lists[draw_list_id];
                draw_list.set_clipping_uniforms();
                let draw_call = &mut draw_list.draw_calls[draw_call_id];
                let _sh = &self.shaders[draw_call.shader_id];
                let _shc = &self.compiled_shaders[draw_call.shader_id];
                
                if draw_call.instance_dirty {
                    draw_call.instance_dirty = false;
                    // update the instance buffer data
                }
                if draw_call.uniforms_dirty {
                    draw_call.uniforms_dirty = false;
                }
                

            }
        }
    }
    
    pub fn repaint(&mut self) {
    }
    
    fn resize_layer_to_turtle(&mut self) {
    }
    
    pub fn event_loop<F>(&mut self, mut event_handler: F)
    where F: FnMut(&mut Cx, &mut Event),
    {
        self.feature = "dx11".to_string();
        
        let mut windows_window = WindowsWindow {..Default::default()};
        
        let mut root_view = View::<NoScrollBar> {
            ..Style::style(self)
        };
        
        self.hlsl_compile_all_shaders();
        
        self.load_binary_deps_from_file();
        
        self.call_event_handler(&mut event_handler, &mut Event::Construct);
        
        self.redraw_area(Area::All);
        
        while self.running {
            //println!("{}{} ",self.playing_anim_areas.len(), self.redraw_areas.len());
            windows_window.poll_events(
                self.playing_anim_areas.len() == 0 && self.redraw_areas.len() == 0 && self.next_frame_callbacks.len() == 0,
                | _events | {
                }
            );
            
            if self.playing_anim_areas.len() != 0 {
                let time = windows_window.time_now();
                // keeps the error as low as possible
                self.call_animation_event(&mut event_handler, time);
            }
            
            if self.next_frame_callbacks.len() != 0 {
                let time = windows_window.time_now();
                // keeps the error as low as possible
                self.call_frame_event(&mut event_handler, time);
            }
            
            self.call_signals_before_draw(&mut event_handler);
            
            // call redraw event
            if self.redraw_areas.len()>0 {
                //let time_start = cocoa_window.time_now();
                self.call_draw_event(&mut event_handler, &mut root_view);
                self.paint_dirty = true;
                //let time_end = cocoa_window.time_now();
                //println!("Redraw took: {}", (time_end - time_start));
            }
            
            self.process_desktop_file_read_requests(&mut event_handler);
            
            self.call_signals_after_draw(&mut event_handler);
            
            // set a cursor
            if !self.down_mouse_cursor.is_none() {
                windows_window.set_mouse_cursor(self.down_mouse_cursor.as_ref().unwrap().clone())
            }
            else if !self.hover_mouse_cursor.is_none() {
                windows_window.set_mouse_cursor(self.hover_mouse_cursor.as_ref().unwrap().clone())
            }
            else {
                windows_window.set_mouse_cursor(MouseCursor::Default)
            }
            
            if let Some(set_ime_position) = self.platform.set_ime_position {
                self.platform.set_ime_position = None;
                windows_window.ime_spot = set_ime_position;
            }
            
            if let Some(window_position) = self.platform.set_window_position {
                self.platform.set_window_position = None;
                windows_window.set_position(window_position);
                self.window_geom = windows_window.get_window_geom();
            }
            
            if let Some(window_outer_size) = self.platform.set_window_outer_size {
                self.platform.set_window_outer_size = None;
                windows_window.set_outer_size(window_outer_size);
                self.window_geom = windows_window.get_window_geom();
                self.resize_layer_to_turtle();
            }
            
            while self.platform.start_timer.len()>0 {
                let (timer_id, interval, repeats) = self.platform.start_timer.pop().unwrap();
                windows_window.start_timer(timer_id, interval, repeats);
            }
            
            while self.platform.stop_timer.len()>0 {
                let timer_id = self.platform.stop_timer.pop().unwrap();
                windows_window.stop_timer(timer_id);
            }
            
            // repaint everything if we need to
            if self.paint_dirty {
                self.paint_dirty = false;
                self.repaint_id += 1;
                self.repaint();
            }
        }
    }
    
    pub fn show_text_ime(&mut self, x: f32, y: f32) {
        self.platform.set_ime_position = Some(Vec2 {x: x, y: y});
    }
    
    pub fn hide_text_ime(&mut self) {
    }
    
    pub fn set_window_outer_size(&mut self, size: Vec2) {
        self.platform.set_window_outer_size = Some(size);
    }
    
    pub fn set_window_position(&mut self, pos: Vec2) {
        self.platform.set_window_position = Some(pos);
    }
    
    pub fn start_timer(&mut self, interval: f64, repeats: bool) -> Timer {
        self.timer_id += 1;
        self.platform.start_timer.push((self.timer_id, interval, repeats));
        Timer{timer_id:self.timer_id}
    }  
     
    pub fn stop_timer(&mut self, timer:&mut Timer) {
        if timer.timer_id != 0{
            self.platform.stop_timer.push(timer.timer_id);
            timer.timer_id = 0;
        }
    }
    
    pub fn send_signal(signal: Signal, value: u64) {
        WindowsWindow::post_signal(signal.signal_id, value);
    }
 
}

#[derive(Clone, Default)]
pub struct CxPlatform {
    pub uni_cx: Dx11Buffer,
    pub post_id: u64,
    pub set_window_position: Option<Vec2>,
    pub set_window_outer_size: Option<Vec2>,
    pub set_ime_position: Option<Vec2>,
    pub start_timer: Vec<(u64, f64, bool)>,
    pub stop_timer: Vec<(u64)>,
    pub text_clipboard_response: Option<String>,
    pub desktop: CxDesktop
}

#[derive(Clone, Default)]
pub struct DrawListPlatform {
    pub uni_dl: Dx11Buffer
}

#[derive(Default, Clone, Debug)]
pub struct DrawCallPlatform {
    pub uni_dr: Dx11Buffer,
    pub inst_vbuf: Dx11Buffer
}

#[derive(Default, Clone, Debug)]
pub struct Dx11Buffer {
    pub last_written: usize,
}

impl Dx11Buffer {

}


#[derive(Default, Clone)]
pub struct Texture2D {
    pub texture_id: usize,
    pub dirty: bool,
    pub image: Vec<u32>,
    pub width: usize,
    pub height: usize,
   // pub dx11texture: Option<metal::Texture>
}

impl Texture2D {
    pub fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        self.image.resize((width * height) as usize, 0);
        self.dirty = true;
    }
    
    pub fn upload_to_device(&mut self) {
        self.dirty = false;
    }
}