use std::mem;
use std::ptr;

use crate::cx::*;
use std::alloc;

impl Cx{
     pub fn exec_draw_list(&mut self, draw_list_id: usize){
        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
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
                let csh = &self.compiled_shaders[draw_call.shader_id];

                if draw_call.instance_dirty{
                    draw_call.instance_dirty = false;
                    // update the instance buffer data
                    draw_call.resources.check_attached_vao(csh, &mut self.resources);

                    self.resources.from_wasm.alloc_array_buffer(
                        draw_call.resources.inst_vb_id,
                        draw_call.instance.len(),
                        draw_call.instance.as_ptr() as *const f32
                    );
                }

                // update/alloc textures?
                for tex_id in &draw_call.textures_2d{
                    let tex = &mut self.textures_2d[*tex_id as usize];
                    if tex.dirty{
                        tex.upload_to_device(&mut self.resources);
                    }
                }
                self.resources.from_wasm.draw_call(
                    draw_call.shader_id,
                    draw_call.resources.vao_id,
                    &self.uniforms,
                    self.redraw_id as usize, // update once a frame
                    &draw_list.uniforms,
                    draw_list_id, // update on drawlist change
                    &draw_call.uniforms,
                    draw_call.draw_call_id, // update on drawcall id change
                    &draw_call.textures_2d
                );
            }
        }
    }

    pub fn repaint(&mut self){
        self.resources.from_wasm.clear(self.clear_color.x, self.clear_color.y, self.clear_color.z, self.clear_color.w);
        self.prepare_frame();        
        self.exec_draw_list(0);
    }

    // incoming to_wasm. There is absolutely no other entrypoint
    // to general rust codeflow than this function. Only the allocators and init
    pub fn process_to_wasm<F>(&mut self, msg:u32, mut event_handler:F)->u32
    where F: FnMut(&mut Cx, &mut Event)
    {
        // store our view root somewhere
        if self.resources.root_view_ptr == 0{
            self.resources.root_view_ptr = Box::into_raw(
                Box::new(View::<NoScrollBar>{..Style::style(self)})
            ) as u32
        }
        let root_view = unsafe{&mut *(self.resources.root_view_ptr as *mut View<NoScrollBar>)};
        
        let mut to_wasm = ToWasm::from(msg);
        self.resources.from_wasm = FromWasm::new();
        let mut is_animation_frame = false;
        loop{
            let msg_type = to_wasm.mu32();
            match msg_type{
                0=>{ // end
                    break;
                },
                1=>{ // fetch_deps
                    self.resources.from_wasm.set_document_title(&self.title);
                    // compile all the shaders
                    self.resources.from_wasm.log(&self.title);
                    
                    // send the UI our deps, overlap with shadercompiler
                    let mut load_deps = Vec::new();
                    for font in &self.fonts{
                        load_deps.push(font.name.clone());
                    }
                    // other textures, things
                    self.resources.from_wasm.load_deps(load_deps);

                    self.compile_all_webgl_shaders();
                },
                2=>{ // deps_loaded
                    let len = to_wasm.mu32();
                    for _i in 0..len{
                        let name = to_wasm.parse_string();
                        let ptr = to_wasm.mu32();
                        self.binary_deps.push(BinaryDep::new_from_wasm(name, ptr))
                    }
                    
                    // lets load the fonts from binary deps
                    let num_fonts = self.fonts.len();
                    for i in 0..num_fonts{
                        let font_file = self.fonts[i].name.clone();
                        let bin_dep = self.get_binary_dep(&font_file);
                        if let Some(mut bin_dep) = bin_dep{
                            if let Err(msg) = self.load_font_from_binary_dep(&mut bin_dep){
                                self.resources.from_wasm.log(&format!("Error loading font! {}", msg));
                            }
                        }
                    }

                },
                3=>{ // init
                    self.target_size = vec2(to_wasm.mf32(),to_wasm.mf32());
                    self.target_dpi_factor = to_wasm.mf32();
                    self.call_event_handler(&mut event_handler, &mut Event::Construct); 
                    self.dirty_area = Area::All;
                },
                4=>{ // resize
                    self.target_size = vec2(to_wasm.mf32(),to_wasm.mf32());
                    self.target_dpi_factor = to_wasm.mf32();
                    
                    // do our initial redraw and repaint
                    self.dirty_area = Area::All;
                },
                5=>{ // animation_frame
                    is_animation_frame = true;
                    let time = to_wasm.mf64();
                    //log!(self, "{} o clock",time);
                    self.call_animation_event(&mut event_handler, time);
                },
                6=>{ // finger down
                    let x = to_wasm.mf32();
                    let y = to_wasm.mf32();
                    let digit = to_wasm.mu32() as usize;
                    self.fingers_down[digit] = true;
                    self.call_event_handler(&mut event_handler, &mut Event::FingerDown(FingerDownEvent{
                        abs_x:x, 
                        abs_y:y,
                        rel_x:x,
                        rel_y:y,
                        handled:false,
                        digit:digit,
                        is_touch:to_wasm.mu32()>0
                    }));
                },
                7=>{ // finger up
                    let x = to_wasm.mf32();
                    let y = to_wasm.mf32();
                    let digit = to_wasm.mu32() as usize;
                    self.fingers_down[digit] = false;
                    if !self.any_fingers_down(){
                        self.down_mouse_cursor = None;
                    }
                    self.call_event_handler(&mut event_handler, &mut Event::FingerUp(FingerUpEvent{
                        abs_x:x, 
                        abs_y:y,
                        rel_x:x,
                        rel_y:y,
                        abs_start_x:0.,
                        abs_start_y:0.,
                        rel_start_x:0.,
                        rel_start_y:0.,
                        digit:digit,
                        is_over:false,
                        is_touch:to_wasm.mu32()>0
                    }));
                },
                8=>{ // finger move
                    let x = to_wasm.mf32();
                    let y = to_wasm.mf32();
                    self.call_event_handler(&mut event_handler, &mut Event::FingerMove(FingerMoveEvent{
                        abs_x:x, 
                        abs_y:y,
                        rel_x:x,
                        rel_y:y,
                        abs_start_x:0.,
                        abs_start_y:0.,
                        rel_start_x:0.,
                        rel_start_y:0.,
                        is_over:false,
                        digit:to_wasm.mu32() as usize,
                        is_touch:to_wasm.mu32()>0
                    }));
                },
                9=>{ // finger hover
                    let x = to_wasm.mf32();
                    let y = to_wasm.mf32();
                    self.hover_mouse_cursor = None;
                    self.call_event_handler(&mut event_handler, &mut Event::FingerHover(FingerHoverEvent{
                        abs_x:x, 
                        abs_y:y,
                        rel_x:x,
                        rel_y:y,
                        handled:false,
                        hover_state:HoverState::Over
                    }));
                },
                10=>{ // finger scroll
                    let x = to_wasm.mf32();
                    let y = to_wasm.mf32();
                    self.call_event_handler(&mut event_handler, &mut Event::FingerScroll(FingerScrollEvent{
                        abs_x:x, 
                        abs_y:y,
                        rel_x:x,
                        rel_y:y,
                        handled:false,
                        scroll_x:to_wasm.mf32(),
                        scroll_y:to_wasm.mf32(),
                    }));
                },
                _=>{
                    panic!("Message unknown")
                }
            };
        };

        if !self.dirty_area.is_empty(){
            self.call_draw_event(&mut event_handler, root_view);
            self.paint_dirty = true;
        }

        // check if we need to send a cursor
        if !self.down_mouse_cursor.is_none(){
            self.resources.from_wasm.set_mouse_cursor(self.down_mouse_cursor.as_ref().unwrap().clone())
        }
        else if !self.hover_mouse_cursor.is_none(){
            self.resources.from_wasm.set_mouse_cursor(self.hover_mouse_cursor.as_ref().unwrap().clone())
        }else{
            self.resources.from_wasm.set_mouse_cursor(MouseCursor::Default);
        }

        if is_animation_frame && self.paint_dirty{
            self.paint_dirty = false;
            self.repaint();
        }

        // free the received message
        to_wasm.dealloc();
        
        // request animation frame if still need to redraw, or repaint
        // we use request animation frame for that.
        let mut req_anim_frame = false;
        if !self.dirty_area.is_empty(){
            req_anim_frame = true
        }
        else if self.playing_anim_areas.len()> 0 || self.paint_dirty{
            req_anim_frame = true
        }
        if req_anim_frame{
            self.resources.from_wasm.request_animation_frame();
        }

        // mark the end of the message
        self.resources.from_wasm.end();

        //return wasm pointer to caller
        self.resources.from_wasm.wasm_ptr()
    }

    // empty stub
    pub fn event_loop<F>(&mut self, mut _event_handler:F)
    where F: FnMut(&mut Cx, Event),
    { 
    }

    pub fn log(&mut self, val:&str){
        self.resources.from_wasm.log(val)
    }


    pub fn compile_all_webgl_shaders(&mut self){
        for sh in &self.shaders{
            let csh = Self::compile_webgl_shader(self.compiled_shaders.len(), &sh, &mut self.resources);
            if let Ok(csh) = csh{
                self.compiled_shaders.push(CompiledShader{
                    shader_id:self.compiled_shaders.len(),
                    ..csh
                });
            }
            else if let Err(err) = csh{
                self.resources.from_wasm.log(&format!("GOT ERROR: {}", err.msg));
                self.compiled_shaders.push(
                    CompiledShader{..Default::default()}
                )
            }
        };
    }

    pub fn compile_webgl_shader(shader_id:usize, sh:&Shader, resources:&mut CxResources)->Result<CompiledShader, SlErr>{
        let ash = Self::gl_assemble_shader(sh, GLShaderType::WebGL1)?;
        //let shader_id = self.compiled_shaders.len();
        resources.from_wasm.compile_webgl_shader(shader_id, &ash);

        let geom_ib_id = resources.get_free_index_buffer();
        let geom_vb_id = resources.get_free_index_buffer();

        resources.from_wasm.alloc_array_buffer(
            geom_vb_id,
            sh.geometry_vertices.len(),
            sh.geometry_vertices.as_ptr() as *const f32
        );

        resources.from_wasm.alloc_index_buffer(
            geom_ib_id,
            sh.geometry_indices.len(),
            sh.geometry_indices.as_ptr() as *const u32
        );

        let csh = CompiledShader{
            shader_id:0,
            geometry_slots:ash.geometry_slots,
            instance_slots:ash.instance_slots,
            geom_vb_id:geom_vb_id,
            geom_ib_id:geom_ib_id,
            uniforms_cx:ash.uniforms_cx.clone(),
            uniforms_dl:ash.uniforms_dl.clone(),
            uniforms_dr:ash.uniforms_dr.clone(),
            texture_slots:ash.texture_slots.clone(),
            rect_instance_props:ash.rect_instance_props.clone(),
            named_instance_props:ash.named_instance_props.clone(),
            //assembled_shader:ash,
            ..Default::default()
        };

        Ok(csh)
    }

}

#[derive(Default,Clone)]
pub struct CompiledShader{
    pub shader_id: usize,
    pub geom_vb_id: usize,
    pub geom_ib_id: usize,
    pub instance_slots:usize,
    pub geometry_slots:usize,
    pub uniforms_dr: Vec<ShVar>,
    pub uniforms_dl: Vec<ShVar>,
    pub uniforms_cx: Vec<ShVar>,
    pub texture_slots:Vec<ShVar>,
    pub rect_instance_props: RectInstanceProps,
    pub named_instance_props: NamedInstanceProps
}

#[derive(Default,Clone)]
pub struct WebGLTexture2D{
    pub texture_id: usize
}


#[derive(Clone, Default)]
pub struct CxShaders{
    pub compiled_shaders: Vec<CompiledShader>,
    pub shaders: Vec<Shader>,
}

// storage buffers for graphics API related resources
#[derive(Clone)]
pub struct CxResources{
    pub from_wasm:FromWasm,
    pub vertex_buffers:usize,
    pub vertex_buffers_free:Vec<usize>,
    pub index_buffers:usize,
    pub index_buffers_free:Vec<usize>,
    pub vaos:usize,
    pub vaos_free:Vec<usize>,
    pub root_view_ptr:u32
}

impl Default for CxResources{
    fn default()->CxResources{
        CxResources{
            from_wasm:FromWasm::zero(),
            vertex_buffers:1,
            vertex_buffers_free:Vec::new(),
            index_buffers:1,
            index_buffers_free:Vec::new(),
            vaos:1,
            vaos_free:Vec::new(),
            root_view_ptr:0
        }
    }
}

impl CxResources{
    pub fn get_free_vertex_buffer(&mut self)->usize{
        if self.vertex_buffers_free.len() > 0{
            self.vertex_buffers_free.pop().unwrap()
        }
        else{
            self.vertex_buffers += 1;
            self.vertex_buffers
        }
    }
    pub fn get_free_index_buffer(&mut self)->usize{
        if self.index_buffers_free.len() > 0{
            self.index_buffers_free.pop().unwrap()
        }
        else{
            self.index_buffers += 1;
            self.index_buffers
        }
    }
    pub fn get_free_vao(&mut self)->usize{
        if self.vaos_free.len() > 0{
            self.vaos_free.pop().unwrap()
        }
        else{
            self.vaos += 1;
            self.vaos
        }
    }
}

#[derive(Clone, Default)]
pub struct DrawListResources{
}

#[derive(Default,Clone)]
pub struct DrawCallResources{
    pub resource_shader_id:Option<usize>,
    pub vao_id:usize,
    pub inst_vb_id:usize
}

impl DrawCallResources{

    pub fn check_attached_vao(&mut self, csh:&CompiledShader, resources:&mut CxResources){
        if self.resource_shader_id.is_none() || self.resource_shader_id.unwrap() != csh.shader_id{
            self.free(resources); // dont reuse vaos accross shader ids
            
            // create the VAO
            self.resource_shader_id = Some(csh.shader_id);

            // get a free vao ID
            self.vao_id = resources.get_free_vao();
            self.inst_vb_id = resources.get_free_index_buffer();

            resources.from_wasm.alloc_array_buffer(
                self.inst_vb_id,0,0 as *const f32
            );

            resources.from_wasm.alloc_vao(
                csh.shader_id,
                self.vao_id,
                csh.geom_ib_id,
                csh.geom_vb_id,
                self.inst_vb_id,
            );
        }
    }

    fn free(&mut self, resources:&mut CxResources){
        
        if self.vao_id != 0{
            resources.vaos_free.push(self.vao_id);
        }
        if self.inst_vb_id != 0{
            resources.vertex_buffers_free.push(self.inst_vb_id);
        }
        self.vao_id = 0;
        self.inst_vb_id = 0;
    }
}




//  Texture



#[derive(Default,Clone)]
pub struct Texture2D{
    pub texture_id: usize,
    pub dirty:bool,
    pub image: Vec<u32>,
    pub width: usize,
    pub height:usize
}

impl Texture2D{
    pub fn resize(&mut self, width:usize, height:usize){
        self.width = width;
        self.height = height;
        self.image.resize((width * height) as usize, 0);
        self.dirty = true;
    }

    pub fn upload_to_device(&mut self, resources:&mut CxResources){
        resources.from_wasm.alloc_texture(self.texture_id, self.width, self.height, &self.image);
        self.dirty = false;
    }
}




//  Wasm API





#[derive(Clone)]
pub struct FromWasm{
    mu32:*mut u32,
    mf32:*mut f32,
    mf64:*mut f64,
    slots:usize,
    used:isize,
    offset:isize
}

impl FromWasm{
    pub fn zero()->FromWasm{
        FromWasm{
            mu32:0 as *mut u32,
            mf32:0 as *mut f32,
            mf64:0 as *mut f64,
            slots:0,
            used:0,
            offset:0
        }
    }
    pub fn new()->FromWasm{
        unsafe{
            let start_bytes = 4096; 
            let buf = alloc::alloc(alloc::Layout::from_size_align(start_bytes as usize, mem::align_of::<u32>()).unwrap()) as *mut u32;
            (buf as *mut u64).write(start_bytes as u64);
            FromWasm{
                mu32:buf as *mut u32,
                mf32:buf as *mut f32,
                mf64:buf as *mut f64,
                slots:start_bytes>>2,
                used:2,
                offset:0
            }
        }
    }

    // fit enough size for RPC structure with exponential alloc strategy
    // returns position to write to
    fn fit(&mut self, slots:usize){
        unsafe{
            if self.used as usize + slots> self.slots{
                let mut new_slots = usize::max(self.used as usize + slots, self.slots * 2);
                if new_slots&1 != 0{ // f64 align
                    new_slots += 1;
                }
                let new_bytes = new_slots<<2;
                let old_bytes = self.slots<<2;
                let new_buf = alloc::alloc(alloc::Layout::from_size_align(new_bytes as usize, mem::align_of::<u64>()).unwrap()) as *mut u32;
                ptr::copy_nonoverlapping(self.mu32, new_buf, self.slots);
                alloc::dealloc(self.mu32 as *mut u8, alloc::Layout::from_size_align(old_bytes as usize, mem::align_of::<u64>()).unwrap());
                self.slots = new_slots;
                (new_buf as *mut u64).write(new_bytes as u64);
                self.mu32 = new_buf;
                self.mf32 = new_buf as *mut f32;
                self.mf64 = new_buf as *mut f64;
            }
            self.offset = self.used;
            self.used += slots as isize;
        }
    }

    fn check(&mut self){
        if self.offset != self.used{
            panic!("Unequal allocation and writes")
        }
    }

    fn mu32(&mut self, v:u32){
        unsafe{
            self.mu32.offset(self.offset).write(v);
            self.offset += 1;
        }
    }

    fn mf32(&mut self, v:f32){
        unsafe{
            self.mf32.offset(self.offset).write(v);
            self.offset += 1;
        }
    }   
 
    fn mf64(&mut self, v:f64){
        unsafe{
            if self.offset&1 != 0{
                self.offset += 1;
            }
            self.mf64.offset(self.offset>>1).write(v);
            self.offset += 2;
        }
    }

    // end the block and return ownership of the pointer
    pub fn end(&mut self){
        self.fit(1);
        self.mu32(0);
    }

    pub fn wasm_ptr(&self)->u32{
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

    // log a string
    pub fn log(&mut self, msg:&str){
        self.fit(1);
        self.mu32(1);
        self.add_string(msg);
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
   
    pub fn load_deps(&mut self, deps:Vec<String>){
        self.fit(1);
        self.mu32(8);
        self.fit(1);
        self.mu32(deps.len() as u32);
        for dep in deps{
            self.add_string(&dep);
        }
    }

    pub fn alloc_texture(&mut self, texture_id:usize, width:usize, height:usize, data:&Vec<u32>){
        self.fit(5);
        self.mu32(9);
        self.mu32(texture_id as u32);
        self.mu32(width as u32);
        self.mu32(height as u32);
        self.mu32(data.as_ptr() as u32)
    }

    pub fn request_animation_frame(&mut self){
        self.fit(1);
        self.mu32(10);
    }

    pub fn set_document_title(&mut self, title:&str){
        self.fit(1);
        self.mu32(11);
        self.add_string(title);
   }

    pub fn set_mouse_cursor(&mut self, mouse_cursor:MouseCursor){
        self.fit(2);
        self.mu32(12);
        let cursor_id = match mouse_cursor{
			MouseCursor::Hidden=>0,
			MouseCursor::Default=>1,
            MouseCursor::Crosshair=>2,
            MouseCursor::Hand=>3,
            MouseCursor::Arrow=>4,
            MouseCursor::Move=>5,
            MouseCursor::Text=>6,
            MouseCursor::Wait=>7,
            MouseCursor::Help=>8,
            MouseCursor::Progress=>9,
            MouseCursor::NotAllowed=>10,
            MouseCursor::ContextMenu=>11,
            MouseCursor::Cell=>12,
            MouseCursor::VerticalText=>13,
            MouseCursor::Alias=>14,
            MouseCursor::Copy=>15,
            MouseCursor::NoDrop=>16,
            MouseCursor::Grab=>17,
            MouseCursor::Grabbing=>18,
            MouseCursor::AllScroll=>19,
            MouseCursor::ZoomIn=>20,
            MouseCursor::ZoomOut=>21,
            MouseCursor::NResize=>22,
            MouseCursor::NeResize=>23,
            MouseCursor::EResize=>24,
            MouseCursor::SeResize=>25,
            MouseCursor::SResize=>26,
            MouseCursor::SwResize=>27,
            MouseCursor::WResize=>28,
            MouseCursor::NwResize=>29,
            MouseCursor::NsResize=>30,
            MouseCursor::NeswResize=>31,
            MouseCursor::EwResize=>32,
            MouseCursor::NwseResize=>33,
            MouseCursor::ColResize=>34,
			MouseCursor::RowResize=>35,

        };
        self.mu32(cursor_id);
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

}

#[derive(Clone)]
struct ToWasm{
    mu32:*mut u32,
    mf32:*mut f32,
    mf64:*mut f64,
    slots:usize,
    offset:isize
}

impl ToWasm{

    pub fn dealloc(&mut self){
        unsafe{
            alloc::dealloc(self.mu32 as *mut u8, alloc::Layout::from_size_align((self.slots * mem::size_of::<u64>()) as usize, mem::align_of::<u32>()).unwrap());
            self.mu32 = 0 as *mut u32;
            self.mf32 = 0 as *mut f32;
            self.mf64 = 0 as *mut f64;
        }
    }

    pub fn from(buf:u32)->ToWasm{
        unsafe{
            let bytes = (buf as *mut u64).read() as usize;
            ToWasm{
                mu32: buf as *mut u32,
                mf32: buf as *mut f32,
                mf64: buf as *mut f64,
                offset: 2,
                slots: bytes>>2
            }
        }
    }

    fn mu32(&mut self)->u32{
        unsafe{
            let ret = self.mu32.offset(self.offset).read();
            self.offset += 1;
            ret
        }
    }

    fn mf32(&mut self)->f32{
        unsafe{
            let ret = self.mf32.offset(self.offset).read();
            self.offset += 1;
            ret
        }
    }   

    fn mf64(&mut self)->f64{
        unsafe{
            if self.offset&1 != 0{
                self.offset +=1;
            }
            let ret = self.mf64.offset(self.offset>>1).read();
            self.offset += 2;
            ret
        }
    }   

    fn parse_string(&mut self)->String{
        let len = self.mu32();
        let mut out = String::new();
        for _i in 0..len{
            if let Some(c) = std::char::from_u32(self.mu32()) {
                out.push(c);
            }
        }
        out
    }
}


// for use with message passing
#[export_name = "alloc_wasm_buffer"]
pub unsafe extern "C" fn alloc_wasm_buffer(bytes:u32)->u32{
    let buf = std::alloc::alloc(std::alloc::Layout::from_size_align(bytes as usize, mem::align_of::<u64>()).unwrap()) as u32;
    (buf as *mut u64).write(bytes as u64);
    buf as u32
}

// for use with message passing
#[export_name = "realloc_wasm_buffer"]
pub unsafe extern "C" fn realloc_wasm_buffer(in_buf:u32, new_bytes:u32)->u32{
    let old_buf = in_buf as *mut u8;
    let old_bytes = (old_buf as *mut u64).read() as usize;
    let new_buf = alloc::alloc(alloc::Layout::from_size_align(new_bytes as usize, mem::align_of::<u64>()).unwrap()) as *mut u8;
    ptr::copy_nonoverlapping(old_buf, new_buf, old_bytes );
    alloc::dealloc(old_buf as *mut u8, alloc::Layout::from_size_align(old_bytes as usize, mem::align_of::<u64>()).unwrap());
    (new_buf as *mut u64).write(new_bytes as u64);
    new_buf as u32
}

#[export_name = "dealloc_wasm_buffer"]
pub unsafe extern "C" fn dealloc_wasm_buffer(in_buf:u32){
    let buf = in_buf as *mut u8;
    let bytes = buf.read() as usize;
    std::alloc::dealloc(buf as *mut u8, std::alloc::Layout::from_size_align(bytes as usize, mem::align_of::<u64>()).unwrap());
}