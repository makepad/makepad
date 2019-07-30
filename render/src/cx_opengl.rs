use crate::cx::*;
use std::ffi::CString;
use std::ptr;
use std::slice;
use std::mem;
use x11_dl::glx;
use x11_dl::glx::{GLXContext, Glx};
use x11_dl::xlib;

impl Cx {
    
    pub fn render_view(&mut self, pass_id: usize, view_id: usize, full_repaint: bool, view_rect: &Rect, opengl_cx: &OpenglCx) {
        
        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
        let draw_calls_len = self.views[view_id].draw_calls_len;
        if !full_repaint && !view_rect.intersects(self.views[view_id].rect) {
            return
        }
        self.views[view_id].set_clipping_uniforms();
        for draw_call_id in 0..draw_calls_len {
            let sub_view_id = self.views[view_id].draw_calls[draw_call_id].sub_view_id;
            if sub_view_id != 0 {
                self.render_view(pass_id, sub_view_id, full_repaint, view_rect, opengl_cx);
            }
            else {
                let cxview = &mut self.views[view_id];
                //view.platform.uni_vw.update_with_f32_data(device, &view.uniforms);
                let draw_call = &mut cxview.draw_calls[draw_call_id];
                let sh = &self.shaders[draw_call.shader_id];
                let shp = sh.platform.as_ref().unwrap();
                
                if draw_call.instance_dirty {
                    draw_call.instance_dirty = false;
                    draw_call.platform.inst_vbuf.update_with_f32_data(opengl_cx, &draw_call.instance);
                }
                
                draw_call.platform.check_vao(draw_call.shader_id, &shp);
                
                if draw_call.uniforms_dirty {
                    draw_call.uniforms_dirty = false;
                }
                
                unsafe {
                    gl::UseProgram(shp.program);
                    gl::BindVertexArray(draw_call.platform.vao.unwrap());
                    let instances = draw_call.instance.len() / sh.mapping.instance_slots;
                    let indices = sh.shader_gen.geometry_indices.len();
                    
                    let cxuniforms = &self.passes[pass_id].uniforms;
                    
                    opengl_cx.set_uniform_buffer(&shp.uniforms_cx, &cxuniforms);
                    opengl_cx.set_uniform_buffer(&shp.uniforms_vw, &cxview.uniforms);
                    opengl_cx.set_uniform_buffer(&shp.uniforms_dr, &draw_call.uniforms);
                    
                    // lets set our textures
                    for (i, texture_id) in draw_call.textures_2d.iter().enumerate() {
                        let cxtexture = &mut self.textures[*texture_id as usize];
                        if cxtexture.update_image {
                            opengl_cx.update_platform_texture_image2d(cxtexture);
                        }
                        // get the loc
                        gl::ActiveTexture(gl::TEXTURE0 + i as u32);
                        gl::BindTexture(gl::TEXTURE_2D, cxtexture.platform.gl_texture.unwrap());
                    }
                    
                    gl::DrawElementsInstanced(
                        gl::TRIANGLES,
                        indices as i32,
                        gl::UNSIGNED_INT,
                        ptr::null(),
                        instances as i32
                    );
                }
            }
        }
    }
    
    fn calc_dirty_bounds(&mut self, pass_id: usize, view_id: usize, view_bounds: &mut ViewBounds) {
        let draw_calls_len = self.views[view_id].draw_calls_len;
        for draw_call_id in 0..draw_calls_len {
            let sub_view_id = self.views[view_id].draw_calls[draw_call_id].sub_view_id;
            if sub_view_id != 0 {
                self.calc_dirty_bounds(pass_id, sub_view_id, view_bounds)
            }
            else {
                let cxview = &mut self.views[view_id];
                let draw_call = &mut cxview.draw_calls[draw_call_id];
                let sh = &self.shaders[draw_call.shader_id];
                let shp = sh.platform.as_ref().unwrap();
                
                if draw_call.instance_dirty {
                    view_bounds.add_rect(&cxview.rect);
                }
            }
        }
    }
    
    
    fn draw_pass_to_window(
        &mut self,
        pass_id: usize,
        _dpi_factor: f32,
        xlib_app: &XlibApp,
        opengl_window: &mut OpenglWindow,
        opengl_cx: &OpenglCx,
    ) {
        let view_id = self.passes[pass_id].main_view_id.unwrap();
        
        let mut view_bounds = ViewBounds::new();
        
        self.calc_dirty_bounds(pass_id, view_id, &mut view_bounds);
        
        let mut full_repaint = view_bounds.max_x - view_bounds.min_x > opengl_window.window_geom.inner_size.x - 100.
            && view_bounds.max_y - view_bounds.min_y > opengl_window.window_geom.inner_size.y - 100.;
        
        let window;
        let view_rect;
        if full_repaint {
            opengl_window.xlib_window.hide_child_windows();
            
            window = opengl_window.xlib_window.window.unwrap();
            
            let pass_size = self.passes[pass_id].pass_size;
            self.passes[pass_id].set_ortho_matrix(Vec2::zero(), pass_size);
            let pix_width = opengl_window.window_geom.inner_size.x * opengl_window.window_geom.dpi_factor;
            let pix_height = opengl_window.window_geom.inner_size.y * opengl_window.window_geom.dpi_factor;
            
            unsafe {
                (opengl_cx.glx.glXMakeCurrent)(xlib_app.display, window, opengl_cx.context);
                gl::Viewport(0, 0, pix_width as i32, pix_height as i32);
            }
            view_rect = Rect::zero();
        }
        else {
            if view_bounds.max_x == std::f32::NEG_INFINITY
                || view_bounds.max_y == std::f32::NEG_INFINITY
                || view_bounds.min_x == std::f32::INFINITY
                || view_bounds.min_x == std::f32::INFINITY
                || view_bounds.min_x == view_bounds.max_x
                || view_bounds.min_y == view_bounds.max_y {
                return
            }
            /*
            unsafe {
                (opengl_cx.glx.glXMakeCurrent)(xlib_app.display, opengl_window.xlib_window.window.unwrap(), opengl_cx.context);
                gl::Viewport(
                    0,
                    0,
                    (opengl_window.window_geom.inner_size.x * opengl_window.window_geom.dpi_factor) as i32,
                    (opengl_window.window_geom.inner_size.y * opengl_window.window_geom.dpi_factor) as i32
                );
                gl::ClearColor(0.0, 1.0, 0.0, 0.0);
                gl::Clear(gl::COLOR_BUFFER_BIT | gl::DEPTH_BUFFER_BIT);
                (opengl_cx.glx.glXSwapBuffers)(xlib_app.display, opengl_window.xlib_window.window.unwrap());
            }*/
            
            let pix_width = ((view_bounds.max_x - view_bounds.min_x) * opengl_window.window_geom.dpi_factor);
            let pix_height = ((view_bounds.max_y - view_bounds.min_y) * opengl_window.window_geom.dpi_factor);
            
            window = opengl_window.xlib_window.alloc_child_window(
                (view_bounds.min_x * opengl_window.window_geom.dpi_factor) as i32,
                (view_bounds.min_y * opengl_window.window_geom.dpi_factor) as i32,
                pix_width as u32,
                pix_height as u32
            ).unwrap();
            
            let pass_size = self.passes[pass_id].pass_size;
            self.passes[pass_id].set_ortho_matrix(
                Vec2 {x: view_bounds.min_x, y: view_bounds.min_y},
                Vec2 {x: pix_width / opengl_window.window_geom.dpi_factor, y: pix_height / opengl_window.window_geom.dpi_factor}
            );
            
            unsafe {
                (opengl_cx.glx.glXMakeCurrent)(xlib_app.display, window, opengl_cx.context);
                gl::Viewport(0, 0, pix_width as i32, pix_height as i32);
            }
            view_rect = Rect {x: view_bounds.min_x, y: view_bounds.min_y, w: view_bounds.max_x - view_bounds.min_x, h: view_bounds.max_y - view_bounds.min_y}
        }
        
        unsafe {
            gl::Disable(gl::DEPTH_TEST);
            gl::BlendEquationSeparate(gl::FUNC_ADD, gl::FUNC_ADD);
            gl::BlendFuncSeparate(gl::ONE, gl::ONE_MINUS_SRC_ALPHA, gl::ONE, gl::ONE_MINUS_SRC_ALPHA);
            gl::Enable(gl::BLEND);
        }
        
        if self.passes[pass_id].color_textures.len()>0 {
            let color_texture = &self.passes[pass_id].color_textures[0];
            if let Some(color) = color_texture.clear_color {
                unsafe {
                    gl::ClearColor(color.r, color.g, color.b, color.a);
                    gl::Clear(gl::COLOR_BUFFER_BIT | gl::DEPTH_BUFFER_BIT);
                }
            }
            else { // dont clear
            }
        }
        else {
            unsafe {
                gl::ClearColor(0.0, 0.0, 0.0, 0.0);
                gl::Clear(gl::COLOR_BUFFER_BIT | gl::DEPTH_BUFFER_BIT);
            }
        }
        self.render_view(pass_id, view_id, full_repaint, &view_rect, &opengl_cx);
        
        unsafe {
            (opengl_cx.glx.glXSwapBuffers)(xlib_app.display, window);
        }
    }
    
    fn draw_pass_to_texture(
        &mut self,
        pass_id: usize,
        _dpi_factor: f32,
        opengl_cx: &OpenglCx,
    ) {
        let view_id = self.passes[pass_id].main_view_id.unwrap();
        let _pass_size = self.passes[pass_id].pass_size;
        
        /*
        for (index, color_texture) in self.passes[pass_id].color_textures.iter().enumerate() {

            let cxtexture = &mut self.textures[color_texture.texture_id];

            metal_cx.update_platform_render_target(cxtexture, dpi_factor, pass_size, false);
            let color_attachment = render_pass_descriptor.color_attachments().object_at(index).unwrap();
            if let Some(mtltex) = &cxtexture.platform.mtltexture {
                color_attachment.set_texture(Some(&mtltex));
            }
            else {
                println!("draw_pass_to_texture invalid render target");
            }
            color_attachment.set_store_action(MTLStoreAction::Store);
            if let Some(color) = color_texture.clear_color {
                color_attachment.set_load_action(MTLLoadAction::Clear);
                color_attachment.set_clear_color(MTLClearColor::new(color.r as f64, color.g as f64, color.b as f64, color.a as f64));
            }
            else {
                color_attachment.set_load_action(MTLLoadAction::Load);
            }
        }
        */
        self.render_view(pass_id, view_id, true, &Rect::zero(), &opengl_cx);
        // commit
    }
    
    pub fn event_loop<F>(&mut self, mut event_handler: F)
    where F: FnMut(&mut Cx, &mut Event),
    {
        self.platform_type = PlatformType::Linux;
        
        let mut xlib_app = XlibApp::new();
        
        xlib_app.init();
        
        let opengl_cx = OpenglCx::new(&xlib_app);
        
        let mut opengl_windows: Vec<OpenglWindow> = Vec::new();
        
        self.opengl_compile_all_shaders(&xlib_app, &opengl_cx);
        
        self.load_fonts_from_file();
        
        self.call_event_handler(&mut event_handler, &mut Event::Construct);
        
        self.redraw_child_area(Area::All);
        
        let mut passes_todo = Vec::new();
        
        xlib_app.event_loop( | xlib_app, events | {
            //let mut paint_dirty = false;
            for mut event in events {
                
                self.process_desktop_pre_event(&mut event, &mut event_handler);
                
                match &event {
                    Event::WindowGeomChange(re) => { // do this here because mac
                        for opengl_window in &mut opengl_windows {if opengl_window.window_id == re.window_id {
                            opengl_window.window_geom = re.new_geom.clone();
                            self.windows[re.window_id].window_geom = re.new_geom.clone();
                            // redraw just this windows root draw list
                            if let Some(main_pass_id) = self.windows[re.window_id].main_pass_id {
                                self.redraw_pass_and_sub_passes(main_pass_id);
                            }
                            break;
                        }}
                        // ok lets not redraw all, just this window
                        self.call_event_handler(&mut event_handler, &mut event);
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
                        self.call_event_handler(&mut event_handler, &mut event);
                    },
                    Event::Paint => {
                        let _vsync = self.process_desktop_paint_callbacks(xlib_app.time_now(), &mut event_handler);
                        
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
                                CxWindowCmd::None => CxWindowCmd::None,
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
                                            
                                            let dpi_factor = opengl_window.window_geom.dpi_factor;
                                            
                                            self.passes[*pass_id].set_dpi_factor(dpi_factor);
                                            
                                            opengl_window.resize_framebuffer(&opengl_cx);
                                            
                                            self.passes[*pass_id].paint_dirty = false;
                                            
                                            self.draw_pass_to_window(
                                                *pass_id,
                                                dpi_factor,
                                                xlib_app,
                                                opengl_window,
                                                &opengl_cx,
                                            );
                                        }}
                                    }
                                    CxPassDepOf::Pass(parent_pass_id) => {
                                        let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                                        self.passes[*pass_id].set_dpi_factor(dpi_factor);
                                        self.draw_pass_to_texture(
                                            *pass_id,
                                            dpi_factor,
                                            &opengl_cx,
                                        );
                                    },
                                    CxPassDepOf::None => ()
                                }
                            }
                        }
                    },
                    Event::None => {
                    },
                    _ => {
                        self.call_event_handler(&mut event_handler, &mut event);
                    }
                }
                if self.process_desktop_post_event(event) {
                    xlib_app.terminate_event_loop();
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
        XlibApp::post_signal(signal.signal_id, value);
    }
    
    pub fn opengl_compile_all_shaders(&mut self, xlib_app: &XlibApp, opengl_cx: &OpenglCx) {
        let xlib = &xlib_app.xlib;
        unsafe {
            let default_screen = (xlib.XDefaultScreen)(xlib_app.display);
            let root_window = (xlib.XRootWindow)(xlib_app.display, default_screen);
            (opengl_cx.glx.glXMakeCurrent)(xlib_app.display, root_window, opengl_cx.context);
        }
        for sh in &mut self.shaders {
            let openglsh = Self::opengl_compile_shader(sh, opengl_cx);
            if let Err(err) = openglsh {
                panic!("Got opengl shader compile error: {}", err.msg);
            }
        };
    }
    
    pub fn opengl_has_shader_error(compile: bool, shader: usize, source: &str) -> Option<String> {
        //None
        unsafe {
            
            let mut success = i32::from(gl::FALSE);
            
            if compile {
                gl::GetShaderiv(shader as u32, gl::COMPILE_STATUS, &mut success);
            }
            else {
                gl::GetProgramiv(shader as u32, gl::LINK_STATUS, &mut success);
            };
            
            if success != i32::from(gl::TRUE) {
                let mut info_log = Vec::<u8>::with_capacity(2048);
                info_log.set_len(2047);
                for i in 0..2047 {
                    info_log[i] = 0;
                };
                if compile {
                    gl::GetShaderInfoLog(shader as u32, 2048, ptr::null_mut(), info_log.as_mut_ptr() as *mut gl::types::GLchar)
                }
                else {
                    gl::GetProgramInfoLog(shader as u32, 2048, ptr::null_mut(), info_log.as_mut_ptr() as *mut gl::types::GLchar)
                }
                let mut r = "".to_string();
                r.push_str(&String::from_utf8(info_log).unwrap());
                r.push_str("\n");
                let split = source.split("\n");
                for (line, chunk) in split.enumerate() {
                    r.push_str(&(line + 1).to_string());
                    r.push_str(":");
                    r.push_str(chunk);
                    r.push_str("\n");
                }
                Some(r)
            }
            else {
                None
            }
        }
    }
    
    pub fn opengl_get_attributes(program: u32, prefix: &str, slots: usize) -> Vec<OpenglAttribute> {
        let mut attribs = Vec::new();
        
        let stride = (slots * mem::size_of::<f32>()) as i32;
        let num_attr = Self::ceil_div4(slots);
        for i in 0..num_attr {
            let mut name = prefix.to_string();
            name.push_str(&i.to_string());
            name.push_str("\0");
            
            let mut size = ((slots - i * 4)) as i32;
            if size > 4 {
                size = 4;
            }
            unsafe {
                attribs.push(
                    OpenglAttribute {
                        loc: gl::GetAttribLocation(program, name.as_ptr() as *const _) as u32,
                        offset: (i * 4 * mem::size_of::<f32>()) as usize,
                        size: size,
                        stride: stride
                    }
                )
            }
        }
        attribs
    }
    
    pub fn opengl_get_uniforms(program: u32, sg: &ShaderGen, unis: &Vec<ShVar>) -> Vec<OpenglUniform> {
        let mut gl_uni = Vec::new();
        
        for uni in unis {
            let mut name0 = "".to_string();
            name0.push_str(&uni.name);
            name0.push_str("\0");
            unsafe {
                gl_uni.push(OpenglUniform {
                    loc: gl::GetUniformLocation(program, name0.as_ptr() as *const _),
                    name: uni.name.clone(),
                    size: sg.get_type_slots(&uni.ty)
                })
            }
        }
        gl_uni
    }
    
    pub fn opengl_get_texture_slots(program: u32, texture_slots: &Vec<ShVar>) -> Vec<OpenglUniform> {
        let mut gl_texture_slots = Vec::new();
        
        for slot in texture_slots {
            let mut name0 = "".to_string();
            name0.push_str(&slot.name);
            name0.push_str("\0");
            unsafe {
                gl_texture_slots.push(OpenglUniform {
                    loc: gl::GetUniformLocation(program, name0.as_ptr() as *const _),
                    name: slot.name.clone(),
                    size: 0
                    //,sampler:sam.sampler.clone()
                })
            }
        }
        gl_texture_slots
    }
    
    pub fn opengl_compile_shader(sh: &mut CxShader, opengl_cx: &OpenglCx) -> Result<(), SlErr> {
        
        let (vertex, fragment, mapping) = Self::gl_assemble_shader(&sh.shader_gen, GLShaderType::OpenGLNoPartialDeriv) ?;
        // now we have a pixel and a vertex shader
        // so lets now pass it to GL
        unsafe {
            let vs = gl::CreateShader(gl::VERTEX_SHADER);
            gl::ShaderSource(vs, 1, [vertex.as_ptr() as *const _].as_ptr(), ptr::null());
            gl::CompileShader(vs);
            if let Some(error) = Self::opengl_has_shader_error(true, vs as usize, &vertex) {
                return Err(SlErr {
                    msg: format!("ERROR::SHADER::VERTEX::COMPILATION_FAILED\n{}", error)
                })
            }
            
            let fs = gl::CreateShader(gl::FRAGMENT_SHADER);
            gl::ShaderSource(fs, 1, [fragment.as_ptr() as *const _].as_ptr(), ptr::null());
            gl::CompileShader(fs);
            if let Some(error) = Self::opengl_has_shader_error(true, fs as usize, &fragment) {
                return Err(SlErr {
                    msg: format!("ERROR::SHADER::FRAGMENT::COMPILATION_FAILED\n{}", error)
                })
            }
            
            let program = gl::CreateProgram();
            gl::AttachShader(program, vs);
            gl::AttachShader(program, fs);
            gl::LinkProgram(program);
            if let Some(error) = Self::opengl_has_shader_error(false, program as usize, "") {
                return Err(SlErr {
                    msg: format!("ERROR::SHADER::LINK::COMPILATION_FAILED\n{}", error)
                })
            }
            gl::DeleteShader(vs);
            gl::DeleteShader(fs);
            
            let geom_attribs = Self::opengl_get_attributes(program, "geomattr", mapping.geometry_slots);
            let inst_attribs = Self::opengl_get_attributes(program, "instattr", mapping.instance_slots);
            
            // lets fetch the uniform positions for our uniforms
            sh.platform = Some(CxPlatformShader {
                program: program,
                geom_ibuf: {
                    let mut buf = OpenglBuffer::default();
                    buf.update_with_u32_data(opengl_cx, &sh.shader_gen.geometry_indices);
                    buf
                },
                geom_vbuf: {
                    let mut buf = OpenglBuffer::default();
                    buf.update_with_f32_data(opengl_cx, &sh.shader_gen.geometry_vertices);
                    buf
                },
                geom_attribs,
                inst_attribs,
                uniforms_cx: Self::opengl_get_uniforms(program, &sh.shader_gen, &mapping.uniforms_cx),
                uniforms_vw: Self::opengl_get_uniforms(program, &sh.shader_gen, &mapping.uniforms_vw),
                uniforms_dr: Self::opengl_get_uniforms(program, &sh.shader_gen, &mapping.uniforms_dr),
            });
            sh.mapping = mapping;
            return Ok(());
        }
    }
}

#[derive(Clone, PartialEq)]
struct ViewBounds {
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32
}

impl ViewBounds {
    fn new() -> ViewBounds {
        ViewBounds {
            min_x: std::f32::INFINITY,
            min_y: std::f32::INFINITY,
            max_x: std::f32::NEG_INFINITY,
            max_y: std::f32::NEG_INFINITY,
        }
    }
    
    fn add_rect(&mut self, rect: &Rect) {
        if rect.x < self.min_x {
            self.min_x = rect.x;
        }
        if rect.x + rect.w > self.max_x {
            self.max_x = rect.x + rect.w;
        }
        if rect.y < self.min_y {
            self.min_y = rect.y;
        }
        if rect.y + rect.h > self.max_y {
            self.max_y = rect.y + rect.h;
        }
    }
}

pub struct OpenglCx {
    pub glx: Glx,
    pub context: GLXContext,
    pub visual_info: *const xlib::XVisualInfo,
}

impl OpenglCx {
    
    pub fn new(xlib_app: &XlibApp) -> OpenglCx {
        let glx = Glx::open().unwrap();
        
        // Load the gl function pointers
        gl::load_with( | symbol | {
            unsafe {
                (glx.glXGetProcAddress)(CString::new(symbol).unwrap().as_bytes_with_nul().as_ptr())
                    .map_or(ptr::null(), | ptr | ptr as *const _)
            }
        });
        
        // start a cx
        // The default screen of the display
        unsafe {
            let default_screen = (xlib_app.xlib.XDefaultScreen)(xlib_app.display);
            
            // Find one or more framebuffer configurations with the given attributes
            let configs = {
                let mut len = 0;
                let ptr = (glx.glXChooseFBConfig)(
                    xlib_app.display,
                    default_screen,
                    [
                        glx::GLX_DOUBLEBUFFER,
                        1,
                        glx::GLX_RENDER_TYPE,
                        glx::GLX_RGBA_BIT,
                        glx::GLX_RED_SIZE,
                        1,
                        glx::GLX_GREEN_SIZE,
                        1,
                        glx::GLX_BLUE_SIZE,
                        1,
                        0,
                    ]
                        .as_mut_ptr(),
                    &mut len,
                );
                slice::from_raw_parts(ptr, len as usize)
            };
            
            // Get a visual that is compatible with the best found framebuffer configuration
            let visual_info = (glx.glXGetVisualFromFBConfig)(xlib_app.display, configs[0]);
            
            // Create a gl context
            let context = (glx.glXCreateContext)(xlib_app.display, visual_info, ptr::null_mut(), 1);
            
            OpenglCx {
                visual_info,
                context,
                glx,
            }
        }
    }
    
    pub fn set_uniform_buffer(&self, locs: &Vec<OpenglUniform>, uni: &Vec<f32>) {
        
        let mut o = 0;
        for loc in locs {
            if o + loc.size > uni.len() {
                return
            }
            if (o & 3) != 0 && (o & 3) + loc.size > 4 { // goes over the boundary
                o += 4 - (o & 3); // make jump to new slot
            }
            if loc.loc >= 0 {
                unsafe {
                    
                    match loc.size {
                        1 => {
                            gl::Uniform1f(loc.loc as i32, uni[o]);
                        },
                        2 => gl::Uniform2f(loc.loc as i32, uni[o], uni[o + 1]),
                        3 => gl::Uniform3f(loc.loc as i32, uni[o], uni[o + 1], uni[o + 2]),
                        4 => {
                            gl::Uniform4f(loc.loc as i32, uni[o], uni[o + 1], uni[o + 2], uni[o + 3]);
                        },
                        16 => {
                            gl::UniformMatrix4fv(loc.loc as i32, 1, 0, uni.as_ptr().offset((o) as isize));
                        },
                        _ => ()
                    }
                }
            };
            o = o + loc.size;
        }
        
    }
    
    
    pub fn update_platform_texture_image2d(&self, cxtexture: &mut CxTexture) {
        
        if cxtexture.desc.width.is_none() || cxtexture.desc.height.is_none() {
            println!("update_platform_texture_image2d without width/height");
            return;
        }
        
        let width = cxtexture.desc.width.unwrap();
        let height = cxtexture.desc.height.unwrap();
        
        // allocate new texture if descriptor change
        if cxtexture.platform.alloc_desc != cxtexture.desc {
            
            cxtexture.platform.alloc_desc = cxtexture.desc.clone();
            cxtexture.platform.width = width as u64;
            cxtexture.platform.height = height as u64;
            
            let mut tex_handle;
            match cxtexture.platform.gl_texture {
                None => {
                    unsafe {
                        tex_handle = mem::uninitialized();
                        gl::GenTextures(1, &mut tex_handle);
                        cxtexture.platform.gl_texture = Some(tex_handle);
                    }
                }
                Some(gl_texture) => {
                    tex_handle = gl_texture
                }
            }
            unsafe {
                gl::BindTexture(gl::TEXTURE_2D, tex_handle);
                gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::LINEAR as i32);
                gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::LINEAR as i32);
                gl::TexImage2D(gl::TEXTURE_2D, 0, gl::RGBA as i32, width as i32, height as i32, 0, gl::RGBA, gl::UNSIGNED_BYTE, cxtexture.image_u32.as_ptr() as *const _);
                gl::BindTexture(gl::TEXTURE_2D, 0);
            }
        }
        
        cxtexture.update_image = false;
    }
}


#[derive(Clone, Default)]
pub struct CxPlatform {
    pub set_window_position: Option<Vec2>,
    pub set_window_outer_size: Option<Vec2>,
    pub set_ime_position: Option<Vec2>,
    pub start_timer: Vec<(u64, f64, bool)>,
    pub stop_timer: Vec<(u64)>,
    pub text_clipboard_response: Option<String>,
    pub desktop: CxDesktop,
}

#[derive(Clone)]
pub struct CxPlatformShader {
    pub program: u32,
    pub geom_vbuf: OpenglBuffer,
    pub geom_ibuf: OpenglBuffer,
    pub geom_attribs: Vec<OpenglAttribute>,
    pub inst_attribs: Vec<OpenglAttribute>,
    pub uniforms_cx: Vec<OpenglUniform>,
    pub uniforms_vw: Vec<OpenglUniform>,
    pub uniforms_dr: Vec<OpenglUniform>,
}


#[derive(Clone)]
struct OpenglWindow {
    pub window_id: usize,
    pub window_geom: WindowGeom,
    pub cal_size: Vec2,
    pub xlib_window: XlibWindow,
}

impl OpenglWindow {
    fn new(window_id: usize, opengl_cx: &OpenglCx, xlib_app: &mut XlibApp, inner_size: Vec2, position: Option<Vec2>, title: &str) -> OpenglWindow {
        
        let mut xlib_window = XlibWindow::new(xlib_app, window_id);
        
        xlib_window.init(title, inner_size, position, opengl_cx.visual_info);
        
        OpenglWindow {
            window_id,
            cal_size: Vec2::zero(),
            window_geom: xlib_window.get_window_geom(),
            xlib_window
        }
    }
    
    fn resize_framebuffer(&mut self, _opengl_cx: &OpenglCx) -> bool {
        let cal_size = Vec2 {
            x: self.window_geom.inner_size.x * self.window_geom.dpi_factor,
            y: self.window_geom.inner_size.y * self.window_geom.dpi_factor
        };
        if self.cal_size != cal_size {
            self.cal_size = cal_size;
            // resize the framebuffer
            true
        }
        else {
            false
        }
    }
    
}

#[derive(Default, Clone)]
pub struct OpenglAttribute {
    pub loc: u32,
    pub size: i32,
    pub offset: usize,
    pub stride: i32
}

#[derive(Default, Clone)]
pub struct OpenglUniform {
    pub loc: i32,
    pub name: String,
    pub size: usize
}
/*
#[derive(Default, Clone)]
pub struct OpenglTextureSlot {
    pub loc: isize,
    pub name: String
}
*/
#[derive(Clone, Default)]
pub struct CxPlatformView {
}

#[derive(Default, Clone, Debug)]
pub struct PlatformDrawCall {
    pub inst_vbuf: OpenglBuffer,
    pub vao_shader_id: Option<usize>,
    pub vao: Option<u32>
}

impl PlatformDrawCall {
    
    pub fn check_vao(&mut self, shader_id: usize, shp: &CxPlatformShader) {
        if self.vao_shader_id.is_none() || self.vao_shader_id.unwrap() != shader_id {
            self.free_vao();
            // create the VAO
            unsafe {
                let mut vao = mem::uninitialized();
                gl::GenVertexArrays(1, &mut vao);
                gl::BindVertexArray(vao);
                
                // bind the vertex and indexbuffers
                gl::BindBuffer(gl::ARRAY_BUFFER, shp.geom_vbuf.gl_buffer.unwrap());
                for attr in &shp.geom_attribs {
                    gl::VertexAttribPointer(attr.loc, attr.size, gl::FLOAT, 0, attr.stride, attr.offset as *const () as *const _);
                    gl::EnableVertexAttribArray(attr.loc);
                }
                
                gl::BindBuffer(gl::ARRAY_BUFFER, self.inst_vbuf.gl_buffer.unwrap());
                
                for attr in &shp.inst_attribs {
                    gl::VertexAttribPointer(attr.loc, attr.size, gl::FLOAT, 0, attr.stride, attr.offset as *const () as *const _);
                    gl::EnableVertexAttribArray(attr.loc);
                    gl::VertexAttribDivisor(attr.loc, 1 as gl::types::GLuint);
                }
                
                // bind the indexbuffer
                gl::BindBuffer(gl::ELEMENT_ARRAY_BUFFER, shp.geom_ibuf.gl_buffer.unwrap());
                gl::BindVertexArray(0);
                
                self.vao_shader_id = Some(shader_id);
                self.vao = Some(vao);
            }
        }
    }
    
    fn free_vao(&mut self) {
        unsafe {
            if let Some(mut vao) = self.vao {
                gl::DeleteVertexArrays(1, &mut vao);
                self.vao = None;
            }
        }
    }
}

#[derive(Default, Clone, Debug)]
pub struct CxPlatformTexture {
    pub alloc_desc: TextureDesc,
    pub width: u64,
    pub height: u64,
    pub gl_texture: Option<u32>
}

#[derive(Default, Clone, Debug)]
pub struct CxPlatformPass {
}

#[derive(Default, Clone, Debug)]
pub struct OpenglBuffer {
    pub gl_buffer: Option<u32>
}

impl OpenglBuffer {
    
    pub fn alloc_gl_buffer(&mut self) {
        unsafe {
            let mut gl_buffer = mem::uninitialized();
            gl::GenBuffers(1, &mut gl_buffer);
            self.gl_buffer = Some(gl_buffer);
        }
    }
    
    pub fn update_with_f32_data(&mut self, _opengl_cx: &OpenglCx, data: &Vec<f32>) {
        if self.gl_buffer.is_none() {
            self.alloc_gl_buffer();
        }
        unsafe {
            gl::BindBuffer(gl::ARRAY_BUFFER, self.gl_buffer.unwrap());
            gl::BufferData(gl::ARRAY_BUFFER, (data.len() * mem::size_of::<f32>()) as gl::types::GLsizeiptr, data.as_ptr() as *const _, gl::STATIC_DRAW);
        }
    }
    
    pub fn update_with_u32_data(&mut self, _opengl_cx: &OpenglCx, data: &Vec<u32>) {
        if self.gl_buffer.is_none() {
            self.alloc_gl_buffer();
        }
        unsafe {
            gl::BindBuffer(gl::ELEMENT_ARRAY_BUFFER, self.gl_buffer.unwrap());
            gl::BufferData(gl::ELEMENT_ARRAY_BUFFER, (data.len() * mem::size_of::<u32>()) as gl::types::GLsizeiptr, data.as_ptr() as *const _, gl::STATIC_DRAW);
        }
    }
}

//use closefds::*;
use std::process::{Child};
//use std::os::unix::process::{CommandExt};

pub fn spawn_process_command(_cmd: &str, _args: &[&str], _current_dir: &str) -> Result<Child, std::io::Error> {
    Err(std::io::Error::new(std::io::ErrorKind::NotFound, ""))
}
