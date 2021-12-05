use crate::cx_xlib::*;
use crate::cx::*;


impl Cx {
    pub fn event_loop<F>(&mut self, mut event_handler: F)
    where F: FnMut(&mut Cx, &mut Event),
    {
        self.event_handler = Some(&mut event_handler as *const dyn FnMut(&mut Cx, &mut Event) as *mut dyn FnMut(&mut Cx, &mut Event));
        self.event_loop_core();
        self.event_handler = None;  
    }

    
    pub fn event_loop_core(&mut self){
        self.platform_type = PlatformType::Linux{custom_window_chrome: LINUX_CUSTOM_WINDOW_CHROME};
        // 
        self.gpu_info.performance = GpuPerformance::Tier1;
        
        let mut xlib_app = XlibApp::new();
        
        xlib_app.init();
        
        let opengl_cx = OpenglCx::new(xlib_app.display);
        
        let mut opengl_windows: Vec<OpenglWindow> = Vec::new();
        
        self.opengl_compile_all_shaders(&opengl_cx);
        
        self.load_all_fonts();  
        
        self.call_event_handler(&mut Event::Construct);
        
        self.redraw_child_area(Area::All);
        
        let mut passes_todo = Vec::new();
        
        xlib_app.event_loop( | xlib_app, events | {
            let mut paint_dirty = false;
            for mut event in events {
                
                self.process_desktop_pre_event(&mut event);
                
                match &event {
                    Event::WindowSetHoverCursor(mc) => {
                        self.set_hover_mouse_cursor(mc.clone());
                    },
                    Event::WindowGeomChange(re) => { // do this here because mac
                        for opengl_window in &mut opengl_windows {if opengl_window.window_id == re.window_id {
                            opengl_window.window_geom = re.new_geom.clone();
                            self.windows[re.window_id].window_geom = re.new_geom.clone();
                            // redraw just this windows root draw list
                            if re.old_geom.inner_size != re.new_geom.inner_size {
                                if let Some(main_pass_id) = self.windows[re.window_id].main_pass_id {
                                    self.redraw_pass_and_sub_passes(main_pass_id);
                                }
                            }
                            break;
                        }}
                        // ok lets not redraw all, just this window
                        self.call_event_handler(&mut event);
                    },
                    Event::WindowClosed(wc) => {
                        // lets remove the window from the set
                        self.windows[wc.window_id].window_state = CxWindowState::Closed;
                        self.windows_free.push(wc.window_id);
                        // remove the d3d11/win32 window
                        
                        for index in 0..opengl_windows.len() {
                            if opengl_windows[index].window_id == wc.window_id {
                                opengl_windows.remove(index);
                                if opengl_windows.len() == 0 {
                                    xlib_app.terminate_event_loop();
                                }
                                for opengl_window in &mut opengl_windows {
                                    opengl_window.xlib_window.update_ptrs();
                                }
                            }
                        }
                        self.call_event_handler(&mut event);
                    },
                    Event::Paint => {
                        let _vsync = self.process_desktop_paint_callbacks(xlib_app.time_now());
                        
                        // construct or destruct windows
                        for (index, window) in self.windows.iter_mut().enumerate() {
                            
                            window.window_state = match &window.window_state {
                                CxWindowState::Create {inner_size, position, title} => {
                                    // lets create a platformwindow
                                    let opengl_window = OpenglWindow::new(index, &opengl_cx, xlib_app, *inner_size, *position, &title);
                                    window.window_geom = opengl_window.window_geom.clone();
                                    opengl_windows.push(opengl_window);
                                    for opengl_window in &mut opengl_windows {
                                        opengl_window.xlib_window.update_ptrs();
                                    }
                                    
                                    CxWindowState::Created
                                },
                                CxWindowState::Close => {
                                    for opengl_window in &mut opengl_windows {if opengl_window.window_id == index {
                                        opengl_window.xlib_window.close_window();
                                        break;
                                    }}
                                    CxWindowState::Closed
                                },
                                CxWindowState::Created => CxWindowState::Created,
                                CxWindowState::Closed => CxWindowState::Closed
                            };
                            
                            window.window_command = match &window.window_command {
                                CxWindowCmd::Restore => {
                                    for opengl_window in &mut opengl_windows {if opengl_window.window_id == index {
                                        opengl_window.xlib_window.restore();
                                    }}
                                    CxWindowCmd::None
                                },
                                CxWindowCmd::Maximize => {
                                    for opengl_window in &mut opengl_windows {if opengl_window.window_id == index {
                                        opengl_window.xlib_window.maximize();
                                    }}
                                    CxWindowCmd::None
                                },
                                CxWindowCmd::Minimize => {
                                    for opengl_window in &mut opengl_windows {if opengl_window.window_id == index {
                                        opengl_window.xlib_window.minimize();
                                    }}
                                    CxWindowCmd::None
                                },
                                _ => CxWindowCmd::None,
                            };
                            
                            if let Some(topmost) = window.window_topmost {
                                for opengl_window in &mut opengl_windows {if opengl_window.window_id == index {
                                    opengl_window.xlib_window.set_topmost(topmost);
                                }}
                            }
                        }
                        // set a cursor
                        if !self.down_mouse_cursor.is_none() {
                            xlib_app.set_mouse_cursor(self.down_mouse_cursor.as_ref().unwrap().clone())
                        }
                        else if !self.hover_mouse_cursor.is_none() {
                            xlib_app.set_mouse_cursor(self.hover_mouse_cursor.as_ref().unwrap().clone())
                        }
                        else {
                            xlib_app.set_mouse_cursor(MouseCursor::Default)
                        }
                        
                        if let Some(set_ime_position) = self.platform.set_ime_position {
                            self.platform.set_ime_position = None;
                            for opengl_window in &mut opengl_windows {
                                opengl_window.xlib_window.set_ime_spot(set_ime_position);
                            }
                        }
                        
                        while self.platform.start_timer.len() > 0 {
                            let (timer_id, interval, repeats) = self.platform.start_timer.pop().unwrap();
                            xlib_app.start_timer(timer_id, interval, repeats);
                        }
                        
                        while self.platform.stop_timer.len() > 0 {
                            let timer_id = self.platform.stop_timer.pop().unwrap();
                            xlib_app.stop_timer(timer_id);
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
                                        for opengl_window in &mut opengl_windows {if opengl_window.window_id == window_id {
                                            if opengl_window.xlib_window.window.is_none() {
                                                break;
                                            }
                                            let dpi_factor = opengl_window.window_geom.dpi_factor;
                                            
                                            self.passes[*pass_id].set_dpi_factor(dpi_factor);
                                            
                                            opengl_window.resize_framebuffer(&opengl_cx);
                                            
                                            self.passes[*pass_id].paint_dirty = false;
                                            
                                            if self.draw_pass_to_window(
                                                *pass_id,
                                                dpi_factor,
                                                opengl_window,
                                                &opengl_cx,
                                                false
                                            ) {
                                                // paint it again a few times, apparently this is necessary
                                                self.passes[*pass_id].paint_dirty = true;
                                                paint_dirty = true;
                                            }
                                            if opengl_window.first_draw {
                                                opengl_window.first_draw = false;
                                                if dpi_factor != self.default_dpi_factor {
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
                                            &opengl_cx,
                                        );
                                    },
                                    CxPassDepOf::None => {
                                        self.draw_pass_to_texture(
                                            *pass_id,
                                            1.0,
                                            &opengl_cx,
                                        );
                                    }
                                }
                            }
                        }
                    },
                    Event::None => {
                    },
                    Event::Signal {..} => {
                        self.call_event_handler(&mut event);
                        self.call_signals_and_triggers();
                    },
                    _ => {
                        self.call_event_handler(&mut event);
                    }
                }
                if self.process_desktop_post_event(event) {
                    xlib_app.terminate_event_loop();
                }
            }
            
            if self.live_styles.changed_live_bodies.len()>0 || self.live_styles.changed_deps.len()>0{
                let changed_live_bodies = self.live_styles.changed_live_bodies.clone();
                let mut errors = self.process_live_styles_changes();
                self.opengl_update_all_shaders(&opengl_cx, &mut errors);
                self.call_live_recompile_event(changed_live_bodies, errors);
            }
            
            self.process_live_style_errors();

            if paint_dirty 
                || self.playing_animator_ids.len() != 0
                || self.redraw_parent_areas.len() != 0
                || self.redraw_child_areas.len() != 0
                || self.next_frames.len() != 0 {
                false
            } else {
                true
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
    
    pub fn post_signal(signal: Signal, status: StatusId) {
        XlibApp::post_signal(signal, status);
    }
    
    pub fn update_menu(&mut self, _menu: &Menu) {
    }
}

#[derive(Clone, Default)]
pub struct CxPlatform {
    pub set_window_position: Option<Vec2>,
    pub set_window_outer_size: Option<Vec2>,
    pub set_ime_position: Option<Vec2>,
    pub start_timer: Vec<(u64, f64, bool)>,
    pub stop_timer: Vec<u64>,
    pub text_clipboard_response: Option<String>,
    pub desktop: CxDesktop,
}
