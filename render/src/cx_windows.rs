// makepad is win10 only because of dx12 + terminal API
use crate::cx_win32::*;
use crate::cx::*;

impl Cx {
    
    pub fn event_loop<F>(&mut self, mut event_handler: F)
    where F: FnMut(&mut Cx, &mut Event),
    {
        self.platform_type = PlatformType::Windows;
        
        let mut win32_app = Win32App::new();
        
        win32_app.init();
        
        let mut d3d11_windows: Vec<D3d11Window> = Vec::new();
        
        let d3d11_cx = D3d11Cx::new();
        
        self.platform.d3d11_cx = Some(&d3d11_cx);
        
        self.hlsl_compile_all_shaders(&d3d11_cx);
        
        self.load_theme_fonts();
        
        self.call_event_handler(&mut event_handler, &mut Event::Construct);
        
        self.redraw_child_area(Area::All);
        let mut passes_todo = Vec::new();
        
        win32_app.event_loop( | win32_app, events | { //if let Ok(d3d11_cx) = d3d11_cx.lock(){
            // acquire d3d11_cx exclusive
            for mut event in events {
                
                self.process_desktop_pre_event(&mut event, &mut event_handler);
                match &event {
                    Event::WindowSetHoverCursor(mc) => {
                        self.set_hover_mouse_cursor(mc.clone());
                    },
                    Event::WindowResizeLoop(wr) => {
                        for d3d11_window in &mut d3d11_windows {
                            if d3d11_window.window_id == wr.window_id {
                                if wr.was_started {
                                    d3d11_window.start_resize();
                                }
                                else {
                                    d3d11_window.stop_resize();
                                }
                            }
                        }
                    },
                    Event::WindowGeomChange(re) => { // do this here because mac
                        for d3d11_window in &mut d3d11_windows {
                            if d3d11_window.window_id == re.window_id {
                                d3d11_window.window_geom = re.new_geom.clone();
                                self.windows[re.window_id].window_geom = re.new_geom.clone();
                                // redraw just this windows root draw list
                                //if re.old_geom.inner_size != re.new_geom.inner_size{
                                    if let Some(main_pass_id) = self.windows[re.window_id].main_pass_id {
                                        self.redraw_pass_and_sub_passes(main_pass_id);
                                    }
                                //}
                                break;
                            }
                        }
                        // ok lets not redraw all, just this window
                        self.call_event_handler(&mut event_handler, &mut event);
                    },
                    Event::WindowClosed(wc) => { // do this here because mac
                        // lets remove the window from the set
                        self.windows[wc.window_id].window_state = CxWindowState::Closed;
                        self.windows_free.push(wc.window_id);
                        // remove the d3d11/win32 window
                        
                        for index in 0..d3d11_windows.len() {
                            if d3d11_windows[index].window_id == wc.window_id {
                                d3d11_windows.remove(index);
                                if d3d11_windows.len() == 0 {
                                    win32_app.terminate_event_loop();
                                }
                                for d3d11_window in &mut d3d11_windows {
                                    d3d11_window.win32_window.update_ptrs();
                                }
                            }
                        }
                        self.call_event_handler(&mut event_handler, &mut event);
                    },
                    Event::Paint => {
                        self.repaint_id += 1;
                        let vsync = self.process_desktop_paint_callbacks(win32_app.time_now(), &mut event_handler);
                        
                        // construct or destruct windows
                        for (index, window) in self.windows.iter_mut().enumerate() {
                            
                            window.window_state = match &window.window_state {
                                CxWindowState::Create {inner_size, position, title} => {
                                    // lets create a platformwindow
                                    let d3d11_window = D3d11Window::new(index, &d3d11_cx, win32_app, *inner_size, *position, &title);
                                    window.window_geom = d3d11_window.window_geom.clone();
                                    d3d11_windows.push(d3d11_window);
                                    for d3d11_window in &mut d3d11_windows {
                                        d3d11_window.win32_window.update_ptrs();
                                    }
                                    CxWindowState::Created
                                },
                                CxWindowState::Close => {
                                    // ok we close the window
                                    // lets send it a WM_CLOSE event
                                    for d3d11_window in &mut d3d11_windows {if d3d11_window.window_id == index {
                                        d3d11_window.win32_window.close_window();
                                        if win32_app.event_loop_running == false{
                                            return false;
                                        }
                                        break;
                                    }}
                                    CxWindowState::Closed
                                },
                                CxWindowState::Created => CxWindowState::Created,
                                CxWindowState::Closed => CxWindowState::Closed
                            };
                            
                            if let Some(set_position) = window.window_set_position {
                                for d3d11_window in &mut d3d11_windows {if d3d11_window.window_id == index {
                                    d3d11_window.win32_window.set_position(set_position);
                                }}
                            }
                            
                            window.window_command = match &window.window_command {
                                CxWindowCmd::Restore => {
                                    for d3d11_window in &mut d3d11_windows {if d3d11_window.window_id == index {
                                        d3d11_window.win32_window.restore();
                                    }}
                                    CxWindowCmd::None
                                },
                                CxWindowCmd::Maximize => {
                                    for d3d11_window in &mut d3d11_windows {if d3d11_window.window_id == index {
                                        d3d11_window.win32_window.maximize();
                                    }}
                                    CxWindowCmd::None
                                },
                                CxWindowCmd::Minimize => {
                                    for d3d11_window in &mut d3d11_windows {if d3d11_window.window_id == index {
                                        d3d11_window.win32_window.minimize();
                                    }}
                                    CxWindowCmd::None
                                },
                                _ => CxWindowCmd::None,
                            };
                            
                            
                            window.window_set_position = None;
                            
                            if let Some(topmost) = window.window_topmost {
                                for d3d11_window in &mut d3d11_windows {if d3d11_window.window_id == index {
                                    d3d11_window.win32_window.set_topmost(topmost);
                                }}
                            }
                        }
                        
                        // set a cursor
                        if !self.down_mouse_cursor.is_none() {
                            win32_app.set_mouse_cursor(self.down_mouse_cursor.as_ref().unwrap().clone())
                        }
                        else if !self.hover_mouse_cursor.is_none() {
                            win32_app.set_mouse_cursor(self.hover_mouse_cursor.as_ref().unwrap().clone())
                        }
                        else {
                            win32_app.set_mouse_cursor(MouseCursor::Default)
                        }
                        
                        if let Some(set_ime_position) = self.platform.set_ime_position {
                            self.platform.set_ime_position = None;
                            for d3d11_window in &mut d3d11_windows {
                                d3d11_window.win32_window.set_ime_spot(set_ime_position);
                            }
                        }
                        
                        while self.platform.start_timer.len() > 0 {
                            let (timer_id, interval, repeats) = self.platform.start_timer.pop().unwrap();
                            win32_app.start_timer(timer_id, interval, repeats);
                        }
                        
                        while self.platform.stop_timer.len() > 0 {
                            let timer_id = self.platform.stop_timer.pop().unwrap();
                            win32_app.stop_timer(timer_id);
                        }
                        
                        // build a list of renderpasses to repaint
                        let mut windows_need_repaint = 0;
                        self.compute_passes_to_repaint(&mut passes_todo, &mut windows_need_repaint);
                        
                        if passes_todo.len() > 0 {
                            for pass_id in &passes_todo {
                                match self.passes[*pass_id].dep_of.clone() {
                                    CxPassDepOf::Window(window_id) => {
                                        // find the accompanying render window
                                        if let Some(d3d11_window) = d3d11_windows.iter_mut().find( | w | w.window_id == window_id) {
                                            windows_need_repaint -= 1;
                                            
                                            let dpi_factor = d3d11_window.window_geom.dpi_factor;
                                            self.passes[*pass_id].set_dpi_factor(dpi_factor);
                                            
                                            d3d11_window.resize_buffers(&d3d11_cx);
                                            
                                            self.draw_pass_to_window(
                                                *pass_id,
                                                vsync,
                                                dpi_factor,
                                                d3d11_window,
                                                &d3d11_cx,
                                            );
                                            // call redraw if we guessed the dpi wrong on startup
                                            if d3d11_window.first_draw{
                                                d3d11_window.first_draw = false;
                                                if dpi_factor != self.default_dpi_factor{
                                                    self.redraw_pass_and_sub_passes(*pass_id);
                                                }
                                            }
                                        }
                                    }
                                    CxPassDepOf::Pass(parent_pass_id) => {
                                        let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                                        self.draw_pass_to_texture(
                                            *pass_id,
                                            dpi_factor,
                                            &d3d11_cx,
                                        );
                                    },
                                    CxPassDepOf::None => {
                                        self.draw_pass_to_texture(
                                            *pass_id,
                                            1.0,
                                            &d3d11_cx,
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
                self.process_desktop_post_event(event);
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
        Win32App::post_signal(signal.signal_id, value);
    }

    pub fn update_menu(&mut self, _menu:&Menu){
    }
}

#[derive(Default)]
pub struct CxPlatform {
    pub uni_cx: D3d11Buffer,
    pub post_id: u64,
    pub set_ime_position: Option<Vec2>,
    pub start_timer: Vec<(u64, f64, bool)>,
    pub stop_timer: Vec<(u64)>,
    pub text_clipboard_response: Option<String>,
    pub desktop: CxDesktop,
    pub d3d11_cx: Option<*const D3d11Cx>
}
