use glutin::dpi::*;
use glutin::GlContext;
use glutin::GlRequest;
use glutin::GlProfile;
use std::mem;
use std::ptr;
use std::ffi::CStr;

pub use crate::cx_shared::*;
use crate::cxshaders::*;
use crate::events::*;

impl Cx{
     pub fn exec_draw_list(&mut self, id: usize){
        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
        for ci in 0..self.drawing.draw_lists[id].draw_calls_len{
            let sub_list_id = self.drawing.draw_lists[id].draw_calls[ci].sub_list_id;
            if sub_list_id != 0{
                self.exec_draw_list(sub_list_id);
            }
            else{
                let draw_list = &self.drawing.draw_lists[id];
                let draw = &draw_list.draw_calls[ci];
                if draw.update_frame_id == self.drawing.frame_id{
                    // update the instance buffer data
                    unsafe{
                        gl::BindBuffer(gl::ARRAY_BUFFER, draw.vao.vb);
                        gl::BufferData(gl::ARRAY_BUFFER,
                                        (draw.instance.len() * mem::size_of::<f32>()) as gl::types::GLsizeiptr,
                                        draw.instance.as_ptr() as *const _, gl::STATIC_DRAW);
                    }
                }

                let sh = &self.shaders.shaders[draw.shader_id];
                let shgl = &self.shaders.compiled_shaders[draw.shader_id];

                unsafe{
                    gl::UseProgram(shgl.program);
                    gl::BindVertexArray(draw.vao.vao);
                    let instances = draw.instance.len() / shgl.instance_slots;
                    let indices = sh.geometry_indices.len();
                    CxShaders::set_uniform_buffer_fallback(&shgl.uniforms_cx, &self.uniforms);
                    CxShaders::set_uniform_buffer_fallback(&shgl.uniforms_dl, &draw_list.uniforms);
                    CxShaders::set_uniform_buffer_fallback(&shgl.uniforms_dr, &draw.uniforms);
                    CxShaders::set_texture_slots(&shgl.texture_slots, &draw.textures, &mut self.textures);
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
            gl::Clear(gl::COLOR_BUFFER_BIT|gl::DEPTH_BUFFER_BIT);
        }

        self.prepare_frame();        
        self.exec_draw_list(0);

        glutin_window.swap_buffers().unwrap();
    }

    fn resize_window_to_turtle(&mut self, glutin_window:&glutin::GlWindow){
       // resize drawable
        glutin_window.resize(PhysicalSize::new(
            (self.turtle.target_size.x * self.turtle.target_dpi_factor) as f64,
            (self.turtle.target_size.y * self.turtle.target_dpi_factor) as f64)
        );
        //gl_window.resize(logical_size.to_physical(dpi_factor));
        //layer.set_drawable_size(CGSize::new(
         //   (self.turtle.target_size.x * self.turtle.target_dpi_factor) as f64,
          //   (self.turtle.target_size.y * self.turtle.target_dpi_factor) as f64));
    }

    pub fn event_loop<F>(&mut self, mut event_handler:F)
    where F: FnMut(&mut Cx, Event),
    { 
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
            gl::ClearColor(0.3, 0.3, 0.3, 1.0);
            gl::Enable(gl::DEPTH_TEST);
            gl::DepthFunc(gl::LEQUAL);
            gl::BlendEquationSeparate(gl::FUNC_ADD, gl::FUNC_ADD);
            gl::BlendFuncSeparate(gl::ONE, gl::ONE_MINUS_SRC_ALPHA, gl::ONE, gl::ONE_MINUS_SRC_ALPHA);
            gl::Enable(gl::BLEND);            

            //let mut num_extensions = 0;
            //gl::GetIntegerv(gl::NUM_EXTENSIONS, &mut num_extensions);
            //let extensions: Vec<_> = (0 .. num_extensions).map(|num| {
            //   Cx::gl_string(gl::GetStringi(gl::EXTENSIONS, num as gl::types::GLuint))
            //}).collect();
            //println!("Extensions   : {}", extensions.join(", "))
        }

        // lets compile all shaders
        self.shaders.compile_all_shaders();

        while self.running{
            events_loop.poll_events(|winit_event|{
                let event = self.map_winit_event(winit_event, &glutin_window);
                if let Event::Resized(_) = &event{
                    self.resize_window_to_turtle(&glutin_window);
                    event_handler(self, event); 
                    self.redraw_all();
                    event_handler(self, Event::Redraw);
                    self.redraw_none();
                    self.repaint(&glutin_window);
                }
                else{
                    event_handler(self, event); 
                }
            });
            // call redraw event
            if let Some(_) = &self.redraw_area{
                event_handler(self, Event::Redraw);
                self.redraw_none();
                self.repaint = true;
            }
            // repaint everything if we need to
            if self.repaint{
                self.repaint(&glutin_window);
                self.repaint = false;
            }

            // wait for the next event
            if self.animations.len() == 0{
                events_loop.run_forever(|winit_event|{
                    let event = self.map_winit_event(winit_event, &glutin_window);
                    if let Event::Resized(_) = &event{
                        self.resize_window_to_turtle(&glutin_window);
                        event_handler(self, event); 
                        event_handler(self, Event::Redraw);
                        self.repaint(&glutin_window);
                    }
                    else{
                        event_handler(self, event);
                    }
                    winit::ControlFlow::Break
                })
            }
        }
    }

    pub fn wasm_msg<F>(&mut self, msg:u32, mut event_handler:F)->u32{
        0
    }
}
