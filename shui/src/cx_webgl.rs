use std::mem;
use std::ptr;

pub use crate::cx_shared::*;
use crate::events::*;
use std::alloc;

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
                   /* 
                    unsafe{
                        gl::BindBuffer(gl::ARRAY_BUFFER, draw.vao.vb);
                        gl::BufferData(gl::ARRAY_BUFFER,
                                        (draw.instance.len() * mem::size_of::<f32>()) as gl::types::GLsizeiptr,
                                        draw.instance.as_ptr() as *const _, gl::STATIC_DRAW);
                    }
                    */
                }

                let sh = &self.shaders.shaders[draw.shader_id];
                let shgl = &self.shaders.compiled_shaders[draw.shader_id];
                /*
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
                */
            }
        }
    }
    
    pub fn repaint(&mut self/*, glutin_window:&glutin::GlWindow*/){
        /*
        unsafe{
            gl::Clear(gl::COLOR_BUFFER_BIT|gl::DEPTH_BUFFER_BIT);
        }*/

        self.prepare_frame();        
        self.exec_draw_list(0);

        //glutin_window.swap_buffers().unwrap();
    }

    fn resize_window_to_turtle(&mut self/*, glutin_window:&glutin::GlWindow*/){
       // resize drawable
        /*glutin_window.resize(PhysicalSize::new(
            (self.turtle.target_size.x * self.turtle.target_dpi_factor) as f64,
            (self.turtle.target_size.y * self.turtle.target_dpi_factor) as f64)
        );*/
        //gl_window.resize(logical_size.to_physical(dpi_factor));
        //layer.set_drawable_size(CGSize::new(
         //   (self.turtle.target_size.x * self.turtle.target_dpi_factor) as f64,
          //   (self.turtle.target_size.y * self.turtle.target_dpi_factor) as f64));
    }

    // incoming wasm_msg
    pub fn wasm_recv<F>(&mut self, msg:u32, mut event_handler:F)->u32{
        let mut wasm_recv = WasmRecv::own(msg);
        self.buffers.wasm_send = WasmSend::new();
        let mut msg = wasm_recv.next_msg();
        loop{
            match msg{
                WasmMsg::Init=>{
                    // compile all the shaders
                    self.shaders.compile_all_shaders(self);
                    self.buffers.wasm_send.log(&self.title);
                },
                WasmMsg::End=>{
                    break;
                }
            };
            msg = wasm_recv.next_msg();
        };
        // return the send message
        self.buffers.wasm_send.end()
    }

    pub fn event_loop<F>(&mut self, mut event_handler:F)
    where F: FnMut(&mut Cx, Event),
    { /*
        let gl_request = GlRequest::Latest;
        let gl_profile = GlProfile::Core;

        let mut events_loop = glutin::EventsLoop::new();
        let window = glutin::WindowBuilder::new()
            .with_title(self.title.clone())
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
        }*/

        // lets compile all shaders
        //self.shaders.compile_all_shaders();
/*
        while self.running{
            /*
            events_loop.poll_events(|winit_event|{
                let event = self.map_winit_event(winit_event, &glutin_window);
                if let Event::Resized(_) = &event{
                    self.resize_window_to_turtle(&glutin_window);
                    event_handler(self, event); 
                    self.redraw_all();
                    event_handler(self, Event::Redraw);
                    self.redraw_clear();
                    self.repaint(&glutin_window);
                }
                else{
                    event_handler(self, event); 
                }
            });*/
            // call redraw event
            if let Some(_) = &self.redraw_area{
                event_handler(self, Event::Redraw);
                self.redraw_none();
                self.repaint = true;
            }
            // repaint everything if we need to
            if self.repaint{
                self.repaint();
                self.repaint = false;
            }

            // wait for the next event
            if self.animations.len() == 0{
                /*
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
                */
            }
        }
        */
    }

}

#[derive(Clone)]
pub struct WasmSend{
    mu32:*mut u32,
    mf32:*mut f32,
    slots:usize,
    used:isize,
    current:isize
}

impl Default for WasmSend{
    fn default()->WasmSend{
        WasmSend{
            mu32:0 as *mut u32,
            mf32:0 as *mut f32,
            slots:0,
            used:0,
            current:0
        }
    }
}

impl WasmSend{
    pub fn new()->WasmSend{
        unsafe{
            let start_slots = 1024; 
            let buf = alloc::alloc(alloc::Layout::from_size_align((start_slots * mem::size_of::<u32>()) as usize, mem::align_of::<u32>()).unwrap()) as *mut u32;
            (buf as *mut u32).write(start_slots as u32);
            WasmSend{
                mu32:buf as *mut u32,
                mf32:buf as *mut f32,
                slots:start_slots,
                used:1,
                current:0
            }
        }
    }

    // fit enough size for RPC structure with exponential alloc strategy
    // returns position to write to
    fn fit(&mut self, slots:usize){
        unsafe{
            if self.used as usize + slots> self.slots{
                let new_slots = usize::max(self.used as usize + slots, self.slots * 2);
                let new_buf = alloc::alloc(alloc::Layout::from_size_align(new_slots * mem::size_of::<u32>(), mem::align_of::<u32>()).unwrap()) as *mut u32;
                ptr::copy_nonoverlapping(self.mu32, new_buf, self.slots);
                alloc::dealloc(self.mu32 as *mut u8, alloc::Layout::from_size_align(self.slots * mem::size_of::<u32>(), mem::align_of::<u32>()).unwrap());
                self.slots = new_slots;
                (new_buf as *mut u32).write(self.slots as u32);
                self.mu32 = new_buf;
                self.mf32 = new_buf as *mut f32;
            }
            self.current = self.used;
            self.used += slots as isize;
        }
    }

    fn check(&mut self){
        if self.current != self.used{
            panic!("Unequal allocation and writes")
        }
    }

    fn mu32(&mut self, v:u32){
        unsafe{
            self.mu32.offset(self.current).write(v);
            self.current += 1;
        }
    }

    fn mf32(&mut self, v:f32){
        unsafe{
            self.mf32.offset(self.current).write(v);
            self.current += 1;
        }
    }   
    
    // end the block and return ownership of the pointer
    pub fn end(&mut self) -> u32{
        self.fit(1);
        self.mu32(0);
        self.mu32 as u32
    }

    pub fn compile_shader(csh:&CompiledShader){
        
    }

    // log a string
    pub fn log(&mut self, msg:&str){
        let len = msg.chars().count();
        self.fit(len + 2);
        self.mu32(1);
        self.mu32(len as u32);
        for c in msg.chars(){
            self.mu32(c as u32);
        }
        self.check();
    }

   
}

#[derive(Clone)]
struct WasmRecv{
    mu32:*mut u32,
    mf32:*mut f32,
    slots:usize,
    parse:isize
}

enum WasmMsg{
    Init,
    End
}

impl WasmRecv{
    pub fn own(buf:u32)->WasmRecv{
        unsafe{
            WasmRecv{
                mu32: buf as *mut u32,
                mf32: buf as *mut f32,
                parse: 1,
                slots: (buf as *mut u32).read() as usize
            }
        }
    }

    pub fn next_msg(&mut self)->WasmMsg{
        unsafe{
            let msgtype = self.mu32.offset(self.parse).read();
            self.parse += 1;
            match msgtype{
                0=>{
                    WasmMsg::End
                },
                1=>{
                    WasmMsg::Init
                },
                _=>{
                    panic!("Unknown message")
                }
            }
        }
    }
}

impl Drop for WasmRecv{
    fn drop(&mut self){
        unsafe{
            alloc::dealloc(self.mu32 as *mut u8, alloc::Layout::from_size_align((self.slots * mem::size_of::<u32>()) as usize, mem::align_of::<u32>()).unwrap());
        }
    }
}

// for use with message passing
#[export_name = "wasm_alloc"]
pub unsafe extern "C" fn wasm_alloc(slots:u32)->u32{
    let buf = std::alloc::alloc(std::alloc::Layout::from_size_align((slots as usize * mem::size_of::<u32>()) as usize, mem::align_of::<u32>()).unwrap()) as u32;
    (buf as *mut u32).write(slots);
    buf as u32
}

// for use with message passing
#[export_name = "wasm_realloc"]
pub unsafe extern "C" fn wasm_realloc(in_buf:u32, new_slots:u32)->u32{
    let old_buf = in_buf as *mut u32;
    let old_slots = old_buf.read() as usize ;
    let new_buf = alloc::alloc(alloc::Layout::from_size_align((new_slots as usize * mem::size_of::<u32>()) as usize, mem::align_of::<u32>()).unwrap()) as *mut u32;
    ptr::copy_nonoverlapping(old_buf, new_buf, old_slots );
    alloc::dealloc(old_buf as *mut u8, alloc::Layout::from_size_align((old_slots * mem::size_of::<u32>()) as usize, mem::align_of::<u32>()).unwrap());
    (new_buf as *mut u32).write(new_slots);
    new_buf as u32
}

#[export_name = "wasm_dealloc"]
pub unsafe extern "C" fn wasm_dealloc(in_buf:u32){
    let buf = in_buf as *mut u32;
    let slots = buf.read() as usize;
    std::alloc::dealloc(buf as *mut u8, std::alloc::Layout::from_size_align((slots * mem::size_of::<u32>()) as usize, mem::align_of::<u32>()).unwrap());
}