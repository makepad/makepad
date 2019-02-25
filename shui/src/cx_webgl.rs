use std::mem;
use std::ptr;

pub use crate::cx_shared::*;
use crate::shader::*;
use crate::cxshaders_gl::*;
use crate::events::*;
use std::alloc;

impl Cx{
     pub fn exec_draw_list(&mut self, draw_list_id: usize){
        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
        for draw_call_id in 0..self.drawing.draw_lists[draw_list_id].draw_calls_len{
            let sub_list_id = self.drawing.draw_lists[draw_list_id].draw_calls[draw_call_id].sub_list_id;
            if sub_list_id != 0{
                self.exec_draw_list(sub_list_id);
            }
            else{ 
                let draw_list = &mut self.drawing.draw_lists[draw_list_id];
                let draw_call = &mut draw_list.draw_calls[draw_call_id];
                let csh = &self.shaders.compiled_shaders[draw_call.shader_id];

                if draw_call.update_frame_id == self.drawing.frame_id{
                    // update the instance buffer data
                    draw_call.resources.check_attached_vao(csh, &mut self.resources);

                    self.resources.wasm_send.alloc_array_buffer(
                        draw_call.resources.inst_vb_id,
                        draw_call.instance.len(),
                        draw_call.instance.as_ptr() as *const f32
                    );
                }
                self.resources.wasm_send.draw_call(
                    draw_call.shader_id,
                    draw_call.resources.vao_id,
                    &self.uniforms,
                    self.drawing.frame_id, // update once a frame
                    &draw_list.uniforms,
                    draw_list_id, // update on drawlist change
                    &draw_call.uniforms,
                    draw_call.draw_call_id, // update on drawcall id change
                    &draw_call.textures
                );
            }
        }
    }

    pub fn clear(&mut self, r:f32, g:f32, b:f32, a:f32){
        self.resources.wasm_send.clear(r,g,b,a);
    }
    
    pub fn repaint(&mut self){
        self.prepare_frame();        
        self.exec_draw_list(0);
    }

    // incoming wasm_msg
    pub fn wasm_recv<F>(&mut self, msg:u32, mut event_handler:F)->u32
    where F: FnMut(&mut Cx, Event)
    {
        let mut wasm_recv = WasmRecv::own(msg);
        self.resources.wasm_send = WasmSend::new();

        loop{
            let msg_type = wasm_recv.mu32();
            match msg_type{
                0=>{ // end
                    break;
                },
                1=>{ // init
                    // render our first pass
                    self.turtle.target_size = vec2(wasm_recv.mf32(),wasm_recv.mf32());
                    self.turtle.target_dpi_factor = wasm_recv.mf32();

                    // compile all the shaders
                    self.resources.wasm_send.log(&self.title);
                    self.shaders.compile_all_webgl_shaders(&mut self.resources);

                    // do our initial redraw and repaint
                    self.redraw_all();
                    event_handler(self, Event::Redraw);
                    self.redraw_none();
                    self.repaint();
                },
                _=>{
                    panic!("Message unknown")
                }
            };
        };
        // return the send message
        self.resources.wasm_send.end()
    }

    pub fn event_loop<F>(&mut self, mut event_handler:F)
    where F: FnMut(&mut Cx, Event),
    { 
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

impl WasmSend{
    pub fn zero()->WasmSend{
        WasmSend{
            mu32:0 as *mut u32,
            mf32:0 as *mut f32,
            slots:0,
            used:0,
            current:0
        }
    }
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

    fn add_shvarvec(&mut self, shvars:&Vec<ShVar>){
        self.fit(1);
        self.mu32(shvars.len() as u32);
        for shvar in shvars{
            self.add_string(&shvar.ty);
            self.add_string(&shvar.name);
        }
    }

    pub fn compile_webgl_shader(&mut self, shader_id:usize, ash:&AssembledGLShader){
        self.fit(2);
        self.mu32(2);
        self.mu32(shader_id as u32);
        self.add_string(&ash.fragment);
        self.add_string(&ash.vertex);
        self.fit(2);
        self.mu32(ash.geometry_slots as u32);
        self.mu32(ash.instance_slots as u32);
        self.add_shvarvec(&ash.uniforms_cx);
        self.add_shvarvec(&ash.uniforms_dl);
        self.add_shvarvec(&ash.uniforms_dr);
        self.add_shvarvec(&ash.texture_slots);
    }   

    pub fn alloc_array_buffer(&mut self, buffer_id:usize, len:usize, data:*const f32){
        self.fit(4);
        self.mu32(3);
        self.mu32(buffer_id as u32);
        self.mu32(len as u32);
        self.mu32(data as u32);
    }

    pub fn alloc_index_buffer(&mut self, buffer_id:usize, len:usize, data:*const u32){
        self.fit(4);
        self.mu32(4);
        self.mu32(buffer_id as u32);
        self.mu32(len as u32);
        self.mu32(data as u32);
    }

    pub fn alloc_vao(&mut self, shader_id:usize, vao_id:usize, geom_ib_id:usize, geom_vb_id:usize, inst_vb_id:usize){
        self.fit(6);
        self.mu32(5);
        self.mu32(shader_id as u32);
        self.mu32(vao_id as u32);
        self.mu32(geom_ib_id as u32);
        self.mu32(geom_vb_id as u32);
        self.mu32(inst_vb_id as u32);
    }

    pub fn draw_call(&mut self, shader_id:usize, vao_id:usize, 
        uniforms_cx:&Vec<f32>, uni_cx_update:usize, 
        uniforms_dl:&Vec<f32>, uni_dl_update:usize,
        uniforms_dr:&Vec<f32>, uni_dr_update:usize,
        textures:&Vec<u32>){
        self.fit(10);
        self.mu32(6);
        self.mu32(shader_id as u32);
        self.mu32(vao_id as u32);
        self.mu32(uniforms_cx.as_ptr() as u32);
        self.mu32(uni_cx_update as u32);
        self.mu32(uniforms_dl.as_ptr() as u32);
        self.mu32(uni_dl_update as u32);
        self.mu32(uniforms_dr.as_ptr() as u32);
        self.mu32(uni_dr_update as u32);
        self.mu32(textures.as_ptr() as u32);
    }

    pub fn clear(&mut self, r:f32, g:f32, b:f32, a:f32){
        self.fit(5);
        self.mu32(7);
        self.mf32(r);
        self.mf32(g);
        self.mf32(b);
        self.mf32(a);
    }

    fn add_string(&mut self, msg:&str){
        let len = msg.chars().count();
        self.fit(len + 1);
        self.mu32(len as u32);
        for c in msg.chars(){
            self.mu32(c as u32);
        }
        self.check();
    }

    // log a string
    pub fn log(&mut self, msg:&str){
        self.fit(1);
        self.mu32(1);
        self.add_string(msg);
    }

   
}

#[derive(Clone)]
struct WasmRecv{
    mu32:*mut u32,
    mf32:*mut f32,
    slots:usize,
    parse:isize
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

    fn mu32(&mut self)->u32{
        unsafe{
            let ret = self.mu32.offset(self.parse).read();
            self.parse += 1;
            ret
        }
    }

    fn mf32(&mut self)->f32{
        unsafe{
            let ret = self.mf32.offset(self.parse).read();
            self.parse += 1;
            ret
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