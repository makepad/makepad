// MacOS specific loop
use crate::cx_cocoa::*;
use crate::cx::*;

impl Cx {
    
    pub fn event_loop<F>(&mut self, mut event_handler: F)
    where F: FnMut(&mut Cx, &mut Event),
    {
        self.platform_type = PlatformType::OSX;
        
        let mut cocoa_app = CocoaApp::new();
        
        cocoa_app.init();
        
        let mut metal_cx = MetalCx::new();
        
        let mut metal_windows: Vec<MetalWindow> = Vec::new();
        
        self.mtl_compile_all_shaders(&metal_cx);
        
        self.load_theme_fonts();
        
        self.call_event_handler(&mut event_handler, &mut Event::Construct);
        
        self.redraw_child_area(Area::All);
        
        let mut passes_todo = Vec::new();
        
        cocoa_app.event_loop( | cocoa_app, events | {
            //let mut paint_dirty = false;
            for mut event in events {
                
                self.process_desktop_pre_event(&mut event, &mut event_handler);
                
                match &event {
                    Event::WindowGeomChange(re) => { // do this here because mac
                        for metal_window in &mut metal_windows {
                            if metal_window.window_id == re.window_id {
                                metal_window.window_geom = re.new_geom.clone();
                                self.windows[re.window_id].window_geom = re.new_geom.clone();
                                // redraw just this windows root draw list
                                if re.old_geom.inner_size != re.new_geom.inner_size{
                                    if let Some(main_pass_id) = self.windows[re.window_id].main_pass_id {
                                        self.redraw_pass_and_sub_passes(main_pass_id);
                                    }
                                }
                                break;
                            }
                        }
                        // ok lets not redraw all, just this window
                        self.call_event_handler(&mut event_handler, &mut event);
                    },
                    Event::WindowClosed(wc) => { 
                        // lets remove the window from the set
                        self.windows[wc.window_id].window_state = CxWindowState::Closed;
                        self.windows_free.push(wc.window_id);
                        // remove the d3d11/win32 window
                        
                        for index in 0..metal_windows.len() {
                            if metal_windows[index].window_id == wc.window_id {
                                metal_windows.remove(index);
                                if metal_windows.len() == 0 {
                                    cocoa_app.terminate_event_loop();
                                }
                                for metal_window in &mut metal_windows {
                                    metal_window.cocoa_window.update_ptrs();
                                }
                            }
                        }
                        self.call_event_handler(&mut event_handler, &mut event);
                    },
                    Event::Paint => {
                        
                        let vsync = self.process_desktop_paint_callbacks(cocoa_app.time_now(), &mut event_handler);
                        
                        // construct or destruct windows
                        for (index, window) in self.windows.iter_mut().enumerate() {
                            
                            window.window_state = match &window.window_state {
                                CxWindowState::Create {inner_size, position, title} => {
                                    // lets create a platformwindow
                                    let metal_window = MetalWindow::new(index, &metal_cx, cocoa_app, *inner_size, *position, &title);
                                    window.window_geom = metal_window.window_geom.clone();
                                    metal_windows.push(metal_window);
                                    for metal_window in &mut metal_windows {
                                        metal_window.cocoa_window.update_ptrs();
                                    }
                                    CxWindowState::Created
                                },
                                CxWindowState::Close => {
                                    for metal_window in &mut metal_windows {if metal_window.window_id == index {
                                        metal_window.cocoa_window.close_window();
                                        break;
                                    }}
                                    CxWindowState::Closed
                                },
                                CxWindowState::Created => CxWindowState::Created,
                                CxWindowState::Closed => CxWindowState::Closed
                            };
                            
                            window.window_command = match &window.window_command {
                                CxWindowCmd::Restore => {
                                    for metal_window in &mut metal_windows {if metal_window.window_id == index {
                                        metal_window.cocoa_window.restore();
                                    }}
                                    CxWindowCmd::None
                                },
                                CxWindowCmd::Maximize => {
                                    for metal_window in &mut metal_windows {if metal_window.window_id == index {
                                        metal_window.cocoa_window.maximize();
                                    }}
                                    CxWindowCmd::None
                                },
                                CxWindowCmd::Minimize => {
                                    for metal_window in &mut metal_windows {if metal_window.window_id == index {
                                        metal_window.cocoa_window.minimize();
                                    }}
                                    CxWindowCmd::None
                                },
                                _ => CxWindowCmd::None,
                            };
                            
                            if let Some(topmost) = window.window_topmost {
                                for metal_window in &mut metal_windows {if metal_window.window_id == index {
                                    metal_window.cocoa_window.set_topmost(topmost);
                                }}
                            }
                        }
                        
                        // set a cursor
                        if !self.down_mouse_cursor.is_none() {
                            cocoa_app.set_mouse_cursor(self.down_mouse_cursor.as_ref().unwrap().clone())
                        }
                        else if !self.hover_mouse_cursor.is_none() {
                            cocoa_app.set_mouse_cursor(self.hover_mouse_cursor.as_ref().unwrap().clone())
                        }
                        else {
                            cocoa_app.set_mouse_cursor(MouseCursor::Default)
                        }
                        
                        if let Some(set_ime_position) = self.platform.set_ime_position {
                            self.platform.set_ime_position = None;
                            for metal_window in &mut metal_windows {
                                metal_window.cocoa_window.set_ime_spot(set_ime_position);
                            }
                        }
                        
                        while self.platform.start_timer.len() > 0 {
                            let (timer_id, interval, repeats) = self.platform.start_timer.pop().unwrap();
                            cocoa_app.start_timer(timer_id, interval, repeats);
                        }
                        
                        while self.platform.stop_timer.len() > 0 {
                            let timer_id = self.platform.stop_timer.pop().unwrap();
                            cocoa_app.stop_timer(timer_id);
                        }
                        
                        if self.platform.set_menu{
                            self.platform.set_menu = false;
                            if let Some(menu) = &self.platform.last_menu{
                                cocoa_app.update_app_menu(menu)
                            }
                        }
                        
                        // build a list of renderpasses to repaint
                        let mut windows_need_repaint = 0;
                        self.compute_passes_to_repaint(&mut passes_todo, &mut windows_need_repaint);
                        
                        if passes_todo.len() > 0 {
                            for pass_id in &passes_todo {
                                match self.passes[*pass_id].dep_of.clone() {
                                    CxPassDepOf::Window(window_id) => {
                                        // find the accompanying render window
                                        // its a render window
                                        windows_need_repaint -= 1;
                                        for metal_window in &mut metal_windows {if metal_window.window_id == window_id {
                                            metal_window.set_vsync_enable(windows_need_repaint == 0 && vsync);
                                            metal_window.set_buffer_count(
                                                if metal_window.window_geom.is_fullscreen {3}else {2}
                                            );
                                            
                                            let dpi_factor = metal_window.window_geom.dpi_factor;
                                            
                                            metal_window.resize_core_animation_layer(&metal_cx);
                                            
                                            self.draw_pass_to_layer(
                                                *pass_id,
                                                dpi_factor,
                                                &metal_window.core_animation_layer,
                                                &mut metal_cx,
                                            );
                                            // call redraw if we guessed the dpi wrong on startup
                                            if metal_window.first_draw{
                                                metal_window.first_draw = false;
                                                if dpi_factor != self.default_dpi_factor{
                                                    self.redraw_pass_and_sub_passes(*pass_id);
                                                }

                                            }
                                        }}
                                    }
                                    CxPassDepOf::Pass(parent_pass_id) => {
                                        let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                                        self.draw_pass_to_texture(
                                            *pass_id,
                                            dpi_factor,
                                            &mut metal_cx,
                                        );
                                    },
                                    CxPassDepOf::None => {
                                        self.draw_pass_to_texture(
                                            *pass_id,
                                            1.0,
                                            &mut metal_cx,
                                        );
                                    }
                                }
                            }
                        }
                    },
                    Event::None => {
                    },
                    Event::Signal{..}=>{
                        self.call_event_handler(&mut event_handler, &mut event);
                        self.call_signals(&mut event_handler);
                    },
                    _ => {
                        self.call_event_handler(&mut event_handler, &mut event);
                    }
                }
                if self.process_desktop_post_event(event) {
                    cocoa_app.terminate_event_loop();
                }
            }
            if self.playing_anim_areas.len() == 0 && self.redraw_parent_areas.len() == 0 && self.redraw_child_areas.len() == 0 && self.frame_callbacks.len() == 0 {
                true
            } else {
                false
            }
        })
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
        Timer {timer_id: self.timer_id}
    }
    
    pub fn stop_timer(&mut self, timer: &mut Timer) {
        if timer.timer_id != 0 {
            self.platform.stop_timer.push(timer.timer_id);
            timer.timer_id = 0;
        }
    }
    
    pub fn post_signal(signal: Signal, value: usize) {
        CocoaApp::post_signal(signal.signal_id, value);
    }
    
    pub fn update_menu(&mut self, menu:&Menu){
        // lets walk the menu and do the cocoa equivalents
        let platform = &mut self.platform;
        if platform.last_menu.is_none() || platform.last_menu.as_ref().unwrap() != menu{
            platform.last_menu = Some(menu.clone());
            platform.set_menu = true;
        }
    }
}

#[derive(Clone, Default)]
pub struct CxPlatform {
    pub bytes_written: usize,
    pub draw_calls_done: usize,
    pub last_menu: Option<Menu>,
    pub set_menu: bool,
    pub set_window_position: Option<Vec2>,
    pub set_window_outer_size: Option<Vec2>,
    pub set_ime_position: Option<Vec2>,
    pub start_timer: Vec<(u64, f64, bool)>,
    pub stop_timer: Vec<(u64)>,
    pub text_clipboard_response: Option<String>,
    pub desktop: CxDesktop,
}