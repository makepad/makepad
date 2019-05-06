use glutin::dpi::*;
use glutin::GlContext;
use glutin::GlRequest;
use glutin::GlProfile;
use std::mem;
use std::ptr;
use std::ffi::CStr;
use time::precise_time_ns;

use crate::cx::*;

impl Cx{


    pub fn exec_draw_list(&mut self, draw_list_id: usize){

        let draw_calls_len = self.draw_lists[draw_list_id].draw_calls_len;

        for draw_call_id in 0..draw_calls_len{
            let sub_list_id = self.draw_lists[draw_list_id].draw_calls[draw_call_id].sub_list_id;
            if sub_list_id != 0{
                self.exec_draw_list(sub_list_id);
            }
            else{
                let draw_list = &mut self.draw_lists[draw_list_id];

                draw_list.set_clipping_uniforms();

                let draw_call = &mut draw_list.draw_calls[draw_call_id];
                let sh = &self.shaders[draw_call.shader_id];
                let csh = &self.compiled_shaders[draw_call.shader_id];

                unsafe{
                    draw_call.platform.check_attached_vao(csh);

                    if draw_call.instance_dirty{
                        draw_call.instance_dirty = false;
                        // update the instance buffer data
                        gl::BindBuffer(gl::ARRAY_BUFFER, draw_call.platform.vb);
                        gl::BufferData(gl::ARRAY_BUFFER,
                                        (draw_call.instance.len() * mem::size_of::<f32>()) as gl::types::GLsizeiptr,
                                        draw_call.instance.as_ptr() as *const _, gl::STATIC_DRAW);
                   }

                    gl::UseProgram(csh.program);
                    gl::BindVertexArray(draw_call.platform.vao);
                    let instances = draw_call.instance.len() / csh.instance_slots;
                    let indices = sh.geometry_indices.len();

                    Cx::set_uniform_buffer_fallback(&csh.uniforms_cx, &self.uniforms);
                    Cx::set_uniform_buffer_fallback(&csh.uniforms_dl, &draw_list.uniforms);
                    Cx::set_uniform_buffer_fallback(&csh.uniforms_dr, &draw_call.uniforms);
                    Cx::set_texture_slots(&csh.texture_slots, &draw_call.textures_2d, &mut self.textures_2d);
                    gl::DrawElementsInstanced(gl::TRIANGLES, indices as i32, gl::UNSIGNED_INT, ptr::null(), instances as i32);
                }
            }
        }
    }

    pub unsafe fn gl_string(raw_string: *const gl::types::GLubyte) -> String {
        if raw_string.is_null() { return "(NULL)".into() }
        String::from_utf8(CStr::from_ptr(raw_string as *const _).to_bytes().to_vec()).ok()
                                    .expect("gl_string: non-UTF8 string")
    }
    
  
    pub fn repaint(&mut self, glutin_window:&glutin::GlWindow){
        unsafe{
            gl::Enable(gl::DEPTH_TEST);
            gl::DepthFunc(gl::LEQUAL);
            gl::BlendEquationSeparate(gl::FUNC_ADD, gl::FUNC_ADD);
            gl::BlendFuncSeparate(gl::ONE, gl::ONE_MINUS_SRC_ALPHA, gl::ONE, gl::ONE_MINUS_SRC_ALPHA);
            gl::Enable(gl::BLEND);
            gl::ClearColor(self.clear_color.r, self.clear_color.g, self.clear_color.b, self.clear_color.a);
            gl::Clear(gl::COLOR_BUFFER_BIT|gl::DEPTH_BUFFER_BIT);
        }
        self.prepare_frame();        
        self.exec_draw_list(0);

        glutin_window.swap_buffers().unwrap();
    }

    fn resize_window_to_turtle(&mut self, glutin_window:&glutin::GlWindow){
        glutin_window.resize(PhysicalSize::new(
            (self.target_size.x * self.target_dpi_factor) as f64,
            (self.target_size.y * self.target_dpi_factor) as f64)
        );
    }
    
    pub fn event_loop<F>(&mut self, mut event_handler:F)
    where F: FnMut(&mut Cx, &mut Event),
    { 
        for _i in 0..10{
             self.platform.fingers_down.push(false);
        }

        let gl_request = GlRequest::Latest;
        let gl_profile = GlProfile::Core;

        let mut events_loop = glutin::EventsLoop::new();
        let window = glutin::WindowBuilder::new()
            .with_title(format!("OpenGL - {}",self.title))
            .with_dimensions(LogicalSize::new(640.0, 480.0));
        let context = glutin::ContextBuilder::new()
            .with_vsync(true)
            .with_gl(gl_request)
            .with_gl_profile(gl_profile);
        let glutin_window = glutin::GlWindow::new(window, context, &events_loop).unwrap();

        unsafe {
            glutin_window.make_current().unwrap();
            gl::load_with(|symbol| glutin_window.get_proc_address(symbol) as *const _);

            //let mut num_extensions = 0;
            //gl::GetIntegerv(gl::NUM_EXTENSIONS, &mut num_extensions);
            //let extensions: Vec<_> = (0 .. num_extensions).map(|num| {
            //   Cx::gl_string(gl::GetStringi(gl::EXTENSIONS, num as gl::types::GLuint))
            //}).collect();
            //println!("Extensions   : {}", extensions.join(", "))
        }

        // lets compile all shaders
        self.compile_all_ogl_shaders();

        let start_time = precise_time_ns();
        let mut root_view = View::<NoScrollBar>{
            ..Style::style(self)
        };
        self.load_binary_deps_from_file();
 
        self.call_event_handler(&mut event_handler, &mut Event::Construct);
        
        self.redraw_area(Area::All);

        while self.running{
            events_loop.poll_events(|winit_event|{
                let mut events = self.map_winit_event(winit_event, &glutin_window);
                for mut event in &mut events{
                    match &event{
                        Event::Resized(_)=>{ // do thi
                            self.resize_window_to_turtle(&glutin_window);
                            self.call_event_handler(&mut event_handler, &mut event); 
                            self.call_draw_event(&mut event_handler, &mut root_view);
                            self.repaint(&glutin_window);
                        },
                        Event::None=>{},
                        _=>{
                            self.call_event_handler(&mut event_handler, &mut event); 
                        }
                    }
                }
            });
            if self.playing_anim_areas.len() != 0{
                let time_now = precise_time_ns();
                let time = (time_now - start_time) as f64 / 1_000_000_000.0; // keeps the error as low as possible
                self.call_animation_event(&mut event_handler, time);
            }

            // call redraw event
            if self.redraw_areas.len()>0{
                self.call_draw_event(&mut event_handler, &mut root_view);
                self.paint_dirty = true;
            }
             
             self.process_desktop_file_read_requests(&mut event_handler);

            // set a cursor
            if !self.down_mouse_cursor.is_none(){
                self.set_winit_mouse_cursor(&glutin_window, self.down_mouse_cursor.as_ref().unwrap().clone())
            }
            else if !self.hover_mouse_cursor.is_none(){
                self.set_winit_mouse_cursor(&glutin_window, self.hover_mouse_cursor.as_ref().unwrap().clone())
            }else{
                self.set_winit_mouse_cursor(&glutin_window, MouseCursor::Default);
            }
            
            // repaint everything if we need to
            if self.paint_dirty{
                self.paint_dirty = false;
                self.repaint(&glutin_window);
            }

            // wait for the next event blockingly so it stops eating power
            if self.playing_anim_areas.len() == 0 && self.redraw_areas.len() == 0{
                events_loop.run_forever(|winit_event|{
                    let mut events = self.map_winit_event(winit_event, &glutin_window);
                    for mut event in &mut events{
                        match &event{
                            Event::Resized(_)=>{ // do thi
                                self.resize_window_to_turtle(&glutin_window);
                                self.call_event_handler(&mut event_handler, &mut event); 
                                self.call_draw_event(&mut event_handler, &mut root_view);
                                self.repaint(&glutin_window);
                            },
                            Event::None=>{},
                            _=>{
                                self.call_event_handler(&mut event_handler, &mut event);
                            }
                        }
                    }
                    winit::ControlFlow::Break
                })
            }
        }
    }

    fn make_mouse_move_events(&self)->Vec<Event>{
        let mut out = Vec::new();
        for i in 0..self.platform.fingers_down.len(){
            let down = self.platform.fingers_down[i];
            if down{
                out.push(Event::FingerMove(FingerMoveEvent{
                    modifiers:KeyModifiers{..Default::default()},
                    abs:self.platform.last_mouse_pos,
                    rel:self.platform.last_mouse_pos,
                    rect:Rect::zero(),
                    digit:i,
                    abs_start:Vec2::zero(),
                    rel_start:Vec2::zero(),
                    is_over:false,
                    is_touch:false,
                    time:0.0
                }))
            }
        };
        return out;
    }

    pub fn set_winit_mouse_cursor(&mut self, window:&winit::Window, mouse_cursor:MouseCursor){
        let (hide, cursor) = match mouse_cursor{
                MouseCursor::Hidden=>(true,winit::MouseCursor::Default),
                MouseCursor::Default=>(false,winit::MouseCursor::Default),
                MouseCursor::Crosshair=>(false,winit::MouseCursor::Crosshair),
                MouseCursor::Hand=>(false,winit::MouseCursor::Hand),
                MouseCursor::Arrow=>(false,winit::MouseCursor::Arrow),
                MouseCursor::Move=>(false,winit::MouseCursor::Move),
                MouseCursor::Text=>(false,winit::MouseCursor::Text),
                MouseCursor::Wait=>(false,winit::MouseCursor::Wait),
                MouseCursor::Help=>(false,winit::MouseCursor::Help),
                MouseCursor::Progress=>(false,winit::MouseCursor::Progress),
                MouseCursor::NotAllowed=>(false,winit::MouseCursor::NotAllowed),
                MouseCursor::ContextMenu=>(false,winit::MouseCursor::ContextMenu),
                MouseCursor::Cell=>(false,winit::MouseCursor::Cell),
                MouseCursor::VerticalText=>(false,winit::MouseCursor::VerticalText),
                MouseCursor::Alias=>(false,winit::MouseCursor::Alias),
                MouseCursor::Copy=>(false,winit::MouseCursor::Copy),
                MouseCursor::NoDrop=>(false,winit::MouseCursor::NoDrop),
                MouseCursor::Grab=>(false,winit::MouseCursor::Grab),
                MouseCursor::Grabbing=>(false,winit::MouseCursor::Grabbing),
                MouseCursor::AllScroll=>(false,winit::MouseCursor::AllScroll),
                MouseCursor::ZoomIn=>(false,winit::MouseCursor::ZoomIn),
                MouseCursor::ZoomOut=>(false,winit::MouseCursor::ZoomOut),
                MouseCursor::NResize=>(false,winit::MouseCursor::NResize),
                MouseCursor::NeResize=>(false,winit::MouseCursor::NeResize),
                MouseCursor::EResize=>(false,winit::MouseCursor::EResize),
                MouseCursor::SeResize=>(false,winit::MouseCursor::SeResize),
                MouseCursor::SResize=>(false,winit::MouseCursor::SResize),
                MouseCursor::SwResize=>(false,winit::MouseCursor::SwResize),
                MouseCursor::WResize=>(false,winit::MouseCursor::WResize),
                MouseCursor::NwResize=>(false,winit::MouseCursor::NwResize),
                MouseCursor::NsResize=>(false,winit::MouseCursor::NsResize),
                MouseCursor::NeswResize=>(false,winit::MouseCursor::NeswResize),
                MouseCursor::EwResize=>(false,winit::MouseCursor::EwResize),
                MouseCursor::NwseResize=>(false,winit::MouseCursor::NwseResize),
                MouseCursor::ColResize=>(false,winit::MouseCursor::ColResize),
                MouseCursor::RowResize=>(false,winit::MouseCursor::RowResize),
        };
        window.set_cursor(cursor);
        window.hide_cursor(hide);
    }

    pub fn map_winit_event(&mut self, winit_event:winit::Event, glutin_window:&winit::Window)->Vec<Event>{
        //self.log(&format!("{:?}\n", winit_event));

        match winit_event{
            winit::Event::DeviceEvent{ event, .. } => match event {
                winit::DeviceEvent::MouseMotion{delta,..}=>{
                    if self.platform.is_cursor_in_window{
                        return vec![Event::None]
                    }
                    self.platform.last_mouse_pos.x += delta.0 as f32;//position.x as f32;
                    self.platform.last_mouse_pos.y += delta.1 as f32;//position.y as f32;
                    
                    return self.make_mouse_move_events();/*vec![Event::FingerHover(FingerHoverEvent{
                        x:self.platform.last_x,
                        y:self.platform.last_y,
                        handled:false,
                        hover_state:HoverState::Over
                    })]*/

                },
                _=>()
            },
            winit::Event::WindowEvent{ event, .. } => match event {
                winit::WindowEvent::ReceivedCharacter(chr)=>{
                    //println!("GOT CHARACTER {}", chr);
                },
                winit::WindowEvent::MouseWheel{delta, ..}=>{
                    let (x, xis_wheel) = match delta{
                        winit::MouseScrollDelta::LineDelta(dx,_dy)=>(-dx*32.0, true),
                        winit::MouseScrollDelta::PixelDelta(pp)=>(-pp.x as f32, false)
                    };
                    let (y, yis_wheel) = match delta{
                        winit::MouseScrollDelta::LineDelta(_dx,dy)=>(-dy*32.0, true),
                        winit::MouseScrollDelta::PixelDelta(pp)=>(-pp.y as f32, false)
                    };
                    return vec![Event::FingerScroll(FingerScrollEvent{
                        modifiers:KeyModifiers{..Default::default()},
                        abs:self.platform.last_mouse_pos,
                        rel:self.platform.last_mouse_pos,
                        rect:Rect::zero(),
                        is_wheel:yis_wheel || xis_wheel,
                        handled:false,
                        scroll:Vec2{x:x, y:y},
                        time:0.0
                    })]
                },
                winit::WindowEvent::CursorMoved{position,..}=>{
                    self.platform.is_cursor_in_window = true;
                    self.platform.last_mouse_pos = Vec2{x:position.x as f32, y:position.y as f32};
                    self.hover_mouse_cursor = None;
                    let mut events = self.make_mouse_move_events();
                    events.push(Event::FingerHover(FingerHoverEvent{
                        abs:self.platform.last_mouse_pos,
                        rel:self.platform.last_mouse_pos,
                        rect:Rect::zero(),
                        modifiers:KeyModifiers{..Default::default()},
                        handled:false,
                        hover_state:HoverState::Over,
                        time:0.0
                    }));
                    return events;
                },
                winit::WindowEvent::CursorEntered{..}=>{
                    self.platform.is_cursor_in_window = true;
                },
                winit::WindowEvent::Focused(state)=>{
                    return vec![Event::AppFocus(state)]
                },
                winit::WindowEvent::CursorLeft{..}=>{
                    self.platform.is_cursor_in_window = false;
                    self.hover_mouse_cursor = None;
                   // fire a hover out on our last known mouse position
                    return vec![Event::FingerHover(FingerHoverEvent{
                        modifiers:KeyModifiers{..Default::default()},
                        abs:self.platform.last_mouse_pos,
                        rel:self.platform.last_mouse_pos,
                        rect:Rect::zero(),
                        handled:false,
                        hover_state:HoverState::Out,
                        time:0.0
                    })]

                },
                winit::WindowEvent::MouseInput{state,button,..}=>{
                    match state{
                        winit::ElementState::Pressed=>{
                            let mut digit = match button{// this makes sure that single touch mode doesnt allow multiple mousedowns
                                winit::MouseButton::Left=>0,
                                winit::MouseButton::Right=>1,
                                winit::MouseButton::Middle=>2,
                                winit::MouseButton::Other(id)=>id as usize
                            };
                            if digit >= self.captured_fingers.len(){
                                digit = 0;
                            };
                            self.platform.fingers_down[digit] = true;
                            return vec![Event::FingerDown(FingerDownEvent{
                                modifiers:KeyModifiers{..Default::default()},
                                abs:self.platform.last_mouse_pos,
                                rel:self.platform.last_mouse_pos,
                                rect:Rect::zero(),
                                handled:false,
                                digit:digit,
                                is_touch:false,
                                time:0.0
                            })]
                        },
                        winit::ElementState::Released=>{

                            let mut digit = match button{// this makes sure that single touch mode doesnt allow multiple mousedowns
                                winit::MouseButton::Left=>0,
                                winit::MouseButton::Right=>1,
                                winit::MouseButton::Middle=>2,
                                winit::MouseButton::Other(id)=>id as usize
                            };
                            if digit >= self.captured_fingers.len(){
                                digit = 0;
                            };
                            self.platform.fingers_down[digit] = false;

                            if !self.platform.fingers_down.iter().any(|down| *down){
                                self.down_mouse_cursor = None;
                            }
                            return vec![Event::FingerUp(FingerUpEvent{
                                modifiers:KeyModifiers{..Default::default()},
                                abs:self.platform.last_mouse_pos,
                                rel:self.platform.last_mouse_pos,
                                rect:Rect::zero(),
                                abs_start:Vec2::zero(),
                                rel_start:Vec2::zero(),
                                digit:digit,
                                is_over:false,
                                is_touch:false,
                                time:0.0
                            })]
                        }
                    }
                },
               
                winit::WindowEvent::CloseRequested =>{
                    self.running = false;
                    return vec![Event::CloseRequested]
                },
                winit::WindowEvent::Resized(logical_size) => {
                    let dpi_factor = glutin_window.get_hidpi_factor();
                    let old_dpi_factor = self.target_dpi_factor as f32;
                    let old_size = self.target_size.clone();
                    self.target_dpi_factor = dpi_factor as f32;
                    self.target_size = Vec2{x:logical_size.width as f32, y:logical_size.height as f32};
                    return vec![Event::Resized(ResizedEvent{
                        old_size: old_size,
                        old_dpi_factor: old_dpi_factor,
                        new_size: self.target_size.clone(),
                        new_dpi_factor: self.target_dpi_factor
                    })]
                },
                _ => ()
            },
            _ => ()
        }
        vec![Event::None]
    }

    pub fn show_text_ime(&mut self, _x:f32, _y:f32){
    }

    pub fn hide_text_ime(&mut self){
        //self.platform.from_wasm.hide_text_ime();
    }

    pub fn compile_all_ogl_shaders(&mut self){
        for sh in &self.shaders{
            let glsh = Self::compile_ogl_shader(&sh);
            if let Ok(glsh) = glsh{
                self.compiled_shaders.push(CompiledShader{
                    shader_id:self.compiled_shaders.len(),
                    ..glsh
                });
            }
            else if let Err(err) = glsh{
                println!("GOT ERROR: {}", err.msg);
                self.compiled_shaders.push(
                    CompiledShader{..Default::default()}
                )
            }
        };
    }

    pub fn compile_has_shader_error(compile:bool, shader:gl::types::GLuint, source:&str)->Option<String>{
        unsafe{
            let mut success = i32::from(gl::FALSE);
           
            if compile{
                gl::GetShaderiv(shader, gl::COMPILE_STATUS, &mut success);
            }
            else{
                gl::GetProgramiv(shader, gl::LINK_STATUS, &mut success);
            };
           
            if success != i32::from(gl::TRUE) {
                 let mut info_log = Vec::<u8>::with_capacity(2048);
                info_log.set_len(2047);
                for i in 0..2047{
                    info_log[i] = 0;
                };
                if compile{
                    gl::GetShaderInfoLog(shader, 2048, ptr::null_mut(),
                        info_log.as_mut_ptr() as *mut gl::types::GLchar)
                }
                else{
                    gl::GetProgramInfoLog(shader, 2048, ptr::null_mut(),
                        info_log.as_mut_ptr() as *mut gl::types::GLchar)
                }
                let mut r = "".to_string();
                r.push_str(&String::from_utf8(info_log).unwrap());
                r.push_str("\n");
                let split = source.split("\n");
                for (line,chunk) in split.enumerate(){
                    r.push_str(&(line+1).to_string());
                    r.push_str(":");
                    r.push_str(chunk);
                    r.push_str("\n");
                }
                Some(r)
            }
            else{
                None
            }
        }
    }

    pub fn compile_get_attributes(program:gl::types::GLuint, prefix:&str, slots:usize)->Vec<GLAttribute>{
        let mut attribs = Vec::new();
        let stride = (slots * mem::size_of::<f32>()) as gl::types::GLsizei;
        let num_attr = Self::ceil_div4(slots);
        for i in 0..num_attr{
            let mut name = prefix.to_string();
            name.push_str(&i.to_string());
            name.push_str("\0");
            
            let mut size = ((slots - i*4)) as gl::types::GLsizei;
            if size > 4{
                size = 4;
            }
            unsafe{
                attribs.push(
                    GLAttribute{
                        loc: gl::GetAttribLocation(program, name.as_ptr() as *const _) as gl::types::GLuint,
                        offset: (i * 4 * mem::size_of::<f32>()) as i32,
                        size:  size,
                        stride: stride
                    }
                )
            }
        }
        attribs
    }

    pub fn compile_get_uniforms(program:gl::types::GLuint, sh:&Shader, unis:&Vec<ShVar>)->Vec<GLUniform>{
        let mut gl_uni = Vec::new();
        for uni in unis{
            let mut name0 = "".to_string();
            name0.push_str(&uni.name);
            name0.push_str("\0");
            unsafe{
                gl_uni.push(GLUniform{
                    loc:gl::GetUniformLocation(program, name0.as_ptr() as *const _),
                    name:uni.name.clone(),
                    size:sh.get_type_slots(&uni.ty)
                })
            }
        }
        gl_uni
    }

    pub fn compile_get_texture_slots(program:gl::types::GLuint, texture_slots:&Vec<ShVar>)->Vec<GLUniform>{
        let mut gl_texture_slots = Vec::new();
        for slot in texture_slots{
            let mut name0 = "".to_string();
            name0.push_str(&slot.name);
            name0.push_str("\0");
            unsafe{
                gl_texture_slots.push(GLUniform{
                    loc:gl::GetUniformLocation(program, name0.as_ptr() as *const _),
                    name:slot.name.clone(),
                    size:0
                    //,sampler:sam.sampler.clone()
                })
            }
        }
        gl_texture_slots
    }

    pub fn compile_ogl_shader(sh:&Shader)->Result<CompiledShader, SlErr>{
        let ash = Self::gl_assemble_shader(sh,GLShaderType::OpenGLNoPartialDeriv)?;
        // now we have a pixel and a vertex shader
        // so lets now pass it to GL
        unsafe{
            
            let vs = gl::CreateShader(gl::VERTEX_SHADER);
            gl::ShaderSource(vs, 1, [ash.vertex.as_ptr() as *const _].as_ptr(), ptr::null());
            gl::CompileShader(vs);
            if let Some(error) = Self::compile_has_shader_error(true, vs, &ash.vertex){
                return Err(SlErr{
                    msg:format!("ERROR::SHADER::VERTEX::COMPILATION_FAILED\n{}",error)
                })
            }

            let fs = gl::CreateShader(gl::FRAGMENT_SHADER);
            gl::ShaderSource(fs, 1, [ash.fragment.as_ptr() as *const _].as_ptr(), ptr::null());
            gl::CompileShader(fs);
            if let Some(error) = Self::compile_has_shader_error(true, fs, &ash.fragment){
                return Err(SlErr{
                    msg:format!("ERROR::SHADER::FRAGMENT::COMPILATION_FAILED\n{}",error)
                })
            }

            let program = gl::CreateProgram();
            gl::AttachShader(program, vs);
            gl::AttachShader(program, fs);
            gl::LinkProgram(program);
            if let Some(error) = Self::compile_has_shader_error(false, program, ""){
                return Err(SlErr{
                    msg:format!("ERROR::SHADER::LINK::COMPILATION_FAILED\n{}",error)
                })
            }
            gl::DeleteShader(vs);
            gl::DeleteShader(fs);

            let geom_attribs = Self::compile_get_attributes(program, "geomattr", ash.geometry_slots);
            let inst_attribs = Self::compile_get_attributes(program, "instattr", ash.instance_slots);

            // lets create static geom and index buffers for this shader
            let mut geom_vb = mem::uninitialized();
            gl::GenBuffers(1, &mut geom_vb);
            gl::BindBuffer(gl::ARRAY_BUFFER, geom_vb);
            gl::BufferData(gl::ARRAY_BUFFER,
                            (sh.geometry_vertices.len() * mem::size_of::<f32>()) as gl::types::GLsizeiptr,
                            sh.geometry_vertices.as_ptr() as *const _, gl::STATIC_DRAW);

            let mut geom_ib = mem::uninitialized();
            gl::GenBuffers(1, &mut geom_ib);
            gl::BindBuffer(gl::ELEMENT_ARRAY_BUFFER, geom_ib);
            gl::BufferData(gl::ELEMENT_ARRAY_BUFFER,
                            (sh.geometry_indices.len() * mem::size_of::<u32>()) as gl::types::GLsizeiptr,
                            sh.geometry_indices.as_ptr() as *const _, gl::STATIC_DRAW);

            // lets fetch the uniform positions for our uniforms
            return Ok(CompiledShader{
                program:program,
                geom_attribs:geom_attribs,
                inst_attribs:inst_attribs,
                geom_vb:geom_vb,
                geom_ib:geom_ib,
                uniforms_cx:Self::compile_get_uniforms(program, sh, &ash.uniforms_cx),
                uniforms_dl:Self::compile_get_uniforms(program, sh, &ash.uniforms_dl),
                uniforms_dr:Self::compile_get_uniforms(program, sh, &ash.uniforms_dr),
                texture_slots:Self::compile_get_texture_slots(program, &ash.texture_slots),
                named_instance_props:ash.named_instance_props.clone(),
                rect_instance_props:ash.rect_instance_props.clone(),
                instance_slots:ash.instance_slots,
                //assembled_shader:ash,
                ..Default::default()
            })
        }
    }


    pub fn set_uniform_buffer_fallback(locs:&Vec<GLUniform>, uni:&Vec<f32>){
        let mut o = 0;
        for loc in locs{
            if o + loc.size > uni.len(){
                return
            }
            if loc.loc >=0 {
                unsafe{
                    match loc.size{
                        1=>gl::Uniform1f(loc.loc, uni[o]),
                        2=>gl::Uniform2f(loc.loc, uni[o], uni[o+1]),
                        3=>gl::Uniform3f(loc.loc, uni[o], uni[o+1], uni[o+2]),
                        4=>gl::Uniform4f(loc.loc, uni[o], uni[o+1], uni[o+2], uni[o+3]),
                        16=>gl::UniformMatrix4fv(loc.loc, 1, 0, uni.as_ptr().offset((o) as isize)),
                        _=>()
                    }
                }
            };
            o = o + loc.size;
        }
    }

    pub fn set_texture_slots(locs:&Vec<GLUniform>, texture_2d_ids:&Vec<u32>, textures_2d:&mut Vec<Texture2D>){
        let mut o = 0;
        for loc in locs{
            let id = texture_2d_ids[o] as usize;
            unsafe{
                gl::ActiveTexture(gl::TEXTURE0 + o as u32);
            }        
            
            if loc.loc >=0{
                let tex = &mut textures_2d[id];
                if tex.dirty{
                    tex.upload_to_device();
                }
                if let Some(gl_texture) = tex.gl_texture{
                    unsafe{
                        gl::BindTexture(gl::TEXTURE_2D, gl_texture);
                    }
                }
            }
            else{
                unsafe{
                    gl::BindTexture(gl::TEXTURE_2D, 0);
                }
            }
            o = o +1;
        }
    }
}


#[derive(Default,Clone)]
pub struct GLAttribute{
    pub loc:gl::types::GLuint,
    pub size:gl::types::GLsizei,
    pub offset:gl::types::GLsizei,
    pub stride:gl::types::GLsizei
}

#[derive(Default,Clone)]
pub struct GLUniform{
    pub loc:gl::types::GLint,
    pub name:String,
    pub size:usize
}

#[derive(Default,Clone)]
pub struct GLTextureSlot{
    pub loc:gl::types::GLint,
    pub name:String
}

#[derive(Default,Clone)]
pub struct AssembledGLShader{
    pub geometry_slots:usize,
    pub instance_slots:usize,
    pub geometry_attribs:usize,
    pub instance_attribs:usize,

    pub uniforms_dr: Vec<ShVar>,
    pub uniforms_dl: Vec<ShVar>,
    pub uniforms_cx: Vec<ShVar>,
    pub texture_slots:Vec<ShVar>,

    pub fragment:String,
    pub vertex:String,
    pub named_instance_props: NamedInstanceProps
}

#[derive(Default,Clone)]
pub struct CompiledShader{
    pub shader_id: usize,
    pub program: gl::types::GLuint,
    pub geom_attribs: Vec<GLAttribute>,
    pub inst_attribs: Vec<GLAttribute>,
    pub geom_vb: gl::types::GLuint,
    pub geom_ib: gl::types::GLuint,
    //pub assembled_shader: AssembledGLShader,
    pub instance_slots:usize,
    pub uniforms_dr: Vec<GLUniform>,
    pub uniforms_dl: Vec<GLUniform>,
    pub uniforms_cx: Vec<GLUniform>,
    pub texture_slots: Vec<GLUniform>,
    pub named_instance_props: NamedInstanceProps,
    pub rect_instance_props: RectInstanceProps
}

#[derive(Default,Clone)]
pub struct GLTexture2D{
    pub texture_id: usize
}

#[derive(Clone, Default)]
pub struct CxShaders{
    pub compiled_shaders: Vec<CompiledShader>,
    pub shaders: Vec<Shader>,
}

#[derive(Clone, Default)]
pub struct CxPlatform{
    pub fingers_down:Vec<bool>,
    pub last_mouse_pos:Vec2,
    pub is_cursor_in_window:bool,
    pub desktop:CxDesktop
}

#[derive(Clone, Default)]
pub struct DrawListPlatform{
}


#[derive(Default,Clone)]
pub struct DrawCallPlatform{
    pub resource_shader_id:Option<usize>,
    pub vao:gl::types::GLuint,
    pub vb:gl::types::GLuint
}

impl DrawCallPlatform{

    pub fn check_attached_vao(&mut self, csh:&CompiledShader){
        if self.resource_shader_id.is_none() || self.resource_shader_id.unwrap() != csh.shader_id{
            self.free();
            // create the VAO
            unsafe{
                self.resource_shader_id = Some(csh.shader_id);
                self.vao = mem::uninitialized();
                gl::GenVertexArrays(1, &mut self.vao);
                gl::BindVertexArray(self.vao);
                
                // bind the vertex and indexbuffers
                gl::BindBuffer(gl::ARRAY_BUFFER, csh.geom_vb);
                for attr in &csh.geom_attribs{
                    gl::VertexAttribPointer(attr.loc, attr.size, gl::FLOAT, 0, attr.stride, attr.offset as *const () as *const _);
                    gl::EnableVertexAttribArray(attr.loc);
                }

                // create and bind the instance buffer
                self.vb = mem::uninitialized();
                gl::GenBuffers(1, &mut self.vb);
                gl::BindBuffer(gl::ARRAY_BUFFER, self.vb);
                
                for attr in &csh.inst_attribs{
                    gl::VertexAttribPointer(attr.loc, attr.size, gl::FLOAT, 0, attr.stride, attr.offset as *const () as *const _);
                    gl::EnableVertexAttribArray(attr.loc);
                    gl::VertexAttribDivisor(attr.loc, 1 as gl::types::GLuint);
                }

                // bind the indexbuffer
                gl::BindBuffer(gl::ELEMENT_ARRAY_BUFFER, csh.geom_ib);
                gl::BindVertexArray(0);
            }
        }
    }

    fn free(&mut self){
        unsafe{
            if self.vao != 0{
                gl::DeleteVertexArrays(1, &mut self.vao);
            }
            if self.vb != 0{
                gl::DeleteBuffers(1, &mut self.vb);
            }
        }
        self.vao = 0;
        self.vb = 0;
    }
}

#[derive(Default,Clone)]
pub struct Texture2D{
    pub texture_id: usize,
    pub dirty:bool,
    pub image: Vec<u32>,
    pub width: usize,
    pub height:usize,
    pub gl_texture: Option<gl::types::GLuint>
}

impl Texture2D{
    pub fn resize(&mut self, width:usize, height:usize){
        self.width = width;
        self.height = height;
        self.image.resize((width * height) as usize, 0);
        self.dirty = true;
    }

    pub fn upload_to_device(&mut self){

        unsafe{
            let mut tex_handle;
            match self.gl_texture{
                None=>{
                    tex_handle = mem::uninitialized();
                    gl::GenTextures(1, &mut tex_handle);
                    self.gl_texture = Some(tex_handle);
                }
                Some(gl_texture)=>{
                    tex_handle = gl_texture
                }
            }
            gl::BindTexture(gl::TEXTURE_2D, tex_handle);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::LINEAR as i32);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::LINEAR as i32);
            gl::TexImage2D(gl::TEXTURE_2D, 0, gl::RGBA as i32, self.width as i32, self.height as i32, 0, gl::RGBA, gl::UNSIGNED_BYTE, self.image.as_ptr() as *const _);
            gl::BindTexture(gl::TEXTURE_2D, 0);
        }

        self.dirty = false;
    }
}
