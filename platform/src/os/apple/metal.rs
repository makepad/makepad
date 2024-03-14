use {
    makepad_objc_sys::{
        msg_send,
        sel,
        class,
        sel_impl,
    },
    crate::{
        makepad_objc_sys::objc_block,
        makepad_shader_compiler::{
            generate_metal,
            generate_metal::MetalGeneratedShader,
        },
        makepad_math::*,
        makepad_live_id::*,
        os::{
            apple::apple_sys::*,
            apple::apple_util::{
                nsstring_to_string,
                str_to_nsstring,
            },
            cx_stdin::PresentableDraw,
        },
        draw_list::DrawListId,
        cx::Cx,
        pass::{PassClearColor, PassClearDepth, PassId},
        studio::{AppToStudio, GPUSample},
        texture::{
            CxTexture,
            Texture,
            TexturePixel,
            TextureFormat,
        },
    },
    std::time::{Instant},
    std::sync::{
        Arc,
        Condvar,
        Mutex,
    },
};

#[cfg(target_os = "macos")]
use crate::{
    metal_xpc::store_xpc_service_texture
};


impl Cx {
    
    
    fn render_view(
        &mut self,
        pass_id: PassId,
        draw_list_id: DrawListId,
        zbias: &mut f32,
        zbias_step: f32,
        encoder: ObjcId,
        command_buffer: ObjcId,
        gpu_read_guards: &mut Vec<MetalRwLockGpuReadGuard>,
        metal_cx: &MetalCx,
    ) {
        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
        let draw_items_len = self.draw_lists[draw_list_id].draw_items.len();
        //self.views[view_id].set_clipping_uniforms();
        self.draw_lists[draw_list_id].uniform_view_transform(&Mat4::identity());
        
        for draw_item_id in 0..draw_items_len {
            if let Some(sub_list_id) = self.draw_lists[draw_list_id].draw_items[draw_item_id].kind.sub_list() {
                self.render_view(
                    pass_id,
                    sub_list_id,
                    zbias,
                    zbias_step,
                    encoder,
                    command_buffer,
                    gpu_read_guards,
                    metal_cx,
                );
            }
            else {
                let draw_list = &mut self.draw_lists[draw_list_id];
                let draw_item = &mut draw_list.draw_items[draw_item_id];
                let draw_call = if let Some(draw_call) = draw_item.kind.draw_call_mut() {
                    draw_call
                }else {
                    continue;
                };
                
                let sh = &self.draw_shaders[draw_call.draw_shader.draw_shader_id];
                if sh.os_shader_id.is_none() { // shader didnt compile somehow
                    continue;
                }
                let shp = &self.draw_shaders.os_shaders[sh.os_shader_id.unwrap()];
                
                if draw_call.instance_dirty {
                    draw_call.instance_dirty = false;
                    // update the instance buffer data
                    self.os.bytes_written += draw_item.instances.as_ref().unwrap().len() * 4;
                    draw_item.os.instance_buffer.next();
                    draw_item.os.instance_buffer.get_mut().cpu_write().update(metal_cx, &draw_item.instances.as_ref().unwrap());
                }
                
                // update the zbias uniform if we have it.
                draw_call.draw_uniforms.set_zbias(*zbias);
                *zbias += zbias_step;
                
                if draw_call.uniforms_dirty {
                    draw_call.uniforms_dirty = false;
                }
                
                // lets verify our instance_offset is not disaligned
                let instances = (draw_item.instances.as_ref().unwrap().len() / sh.mapping.instances.total_slots) as u64;
                
                if instances == 0 {
                    continue;
                }
                let render_pipeline_state = shp.render_pipeline_state.as_id();
                unsafe {let () = msg_send![encoder, setRenderPipelineState: render_pipeline_state];}
                
                let geometry_id = if let Some(geometry_id) = draw_call.geometry_id {geometry_id}
                else {
                    continue;
                };
                
                let geometry = &mut self.geometries[geometry_id];
                
                if geometry.dirty {
                    geometry.os.index_buffer.next();
                    geometry.os.index_buffer.get_mut().cpu_write().update(metal_cx, &geometry.indices);
                    geometry.os.vertex_buffer.next();
                    geometry.os.vertex_buffer.get_mut().cpu_write().update(metal_cx, &geometry.vertices);
                    geometry.dirty = false;
                }
                
                if let Some(inner) = geometry.os.vertex_buffer.get().cpu_read().inner.as_ref() {
                    unsafe {msg_send![
                        encoder,
                        setVertexBuffer: inner.buffer.as_id()
                        offset: 0
                        atIndex: 0
                    ]}
                }
                else {crate::error!("Drawing error: vertex_buffer None")}
                
                if let Some(inner) = draw_item.os.instance_buffer.get().cpu_read().inner.as_ref() {
                    unsafe {msg_send![
                        encoder,
                        setVertexBuffer: inner.buffer.as_id()
                        offset: 0
                        atIndex: 1
                    ]}
                }
                else {crate::error!("Drawing error: instance_buffer None")}
                
                let pass_uniforms = self.passes[pass_id].pass_uniforms.as_slice();
                let draw_list_uniforms = draw_list.draw_list_uniforms.as_slice();
                let draw_uniforms = draw_call.draw_uniforms.as_slice();
                
                unsafe {
                    
                    let () = msg_send![encoder, setVertexBytes: sh.mapping.live_uniforms_buf.as_ptr() as *const std::ffi::c_void length: (sh.mapping.live_uniforms_buf.len() * 4) as u64 atIndex: 2u64];
                    let () = msg_send![encoder, setFragmentBytes: sh.mapping.live_uniforms_buf.as_ptr() as *const std::ffi::c_void length: (sh.mapping.live_uniforms_buf.len() * 4) as u64 atIndex: 2u64];
                    
                    if let Some(id) = shp.draw_uniform_buffer_id {
                        let () = msg_send![encoder, setVertexBytes: draw_uniforms.as_ptr() as *const std::ffi::c_void length: (draw_uniforms.len() * 4) as u64 atIndex: id];
                        let () = msg_send![encoder, setFragmentBytes: draw_uniforms.as_ptr() as *const std::ffi::c_void length: (draw_uniforms.len() * 4) as u64 atIndex: id];
                    }
                    if let Some(id) = shp.pass_uniform_buffer_id {
                        let () = msg_send![encoder, setVertexBytes: pass_uniforms.as_ptr() as *const std::ffi::c_void length: (pass_uniforms.len() * 4) as u64 atIndex: id];
                        let () = msg_send![encoder, setFragmentBytes: pass_uniforms.as_ptr() as *const std::ffi::c_void length: (pass_uniforms.len() * 4) as u64 atIndex: id];
                    }
                    if let Some(id) = shp.view_uniform_buffer_id {
                        let () = msg_send![encoder, setVertexBytes: draw_list_uniforms.as_ptr() as *const std::ffi::c_void length: (draw_list_uniforms.len() * 4) as u64 atIndex: id];
                        let () = msg_send![encoder, setFragmentBytes: draw_list_uniforms.as_ptr() as *const std::ffi::c_void length: (draw_list_uniforms.len() * 4) as u64 atIndex: id];
                    }
                    if let Some(id) = shp.user_uniform_buffer_id {
                        let () = msg_send![encoder, setVertexBytes: draw_call.user_uniforms.as_ptr() as *const std::ffi::c_void length: (draw_call.user_uniforms.len() * 4) as u64 atIndex: id];
                        let () = msg_send![encoder, setFragmentBytes: draw_call.user_uniforms.as_ptr() as *const std::ffi::c_void length: (draw_call.user_uniforms.len() * 4) as u64 atIndex: id];
                    }
                    
                    let ct = &sh.mapping.const_table.table;
                    if ct.len()>0 {
                        let () = msg_send![encoder, setVertexBytes: ct.as_ptr() as *const std::ffi::c_void length: (ct.len() * 4) as u64 atIndex: 3u64];
                        let () = msg_send![encoder, setFragmentBytes: ct.as_ptr() as *const std::ffi::c_void length: (ct.len() * 4) as u64 atIndex: 3u64];
                    }
                }
                // lets set our textures
                for i in 0..sh.mapping.textures.len() {
                    
                    let texture_id = if let Some(texture) = &draw_call.texture_slots[i] {
                        texture.texture_id()
                    }else {
                        let () = unsafe {msg_send![
                            encoder,
                            setFragmentTexture: nil
                            atIndex: i as u64
                        ]};
                        let () = unsafe {msg_send![
                            encoder,
                            setVertexTexture: nil
                            atIndex: i as u64
                        ]};
                        continue
                    };
                    
                    let cxtexture = &mut self.textures[texture_id];
                    
                    if cxtexture.format.is_shared() {
                        #[cfg(target_os = "macos")]
                        cxtexture.update_shared_texture(
                            metal_cx.device,
                        );
                    }
                    else if cxtexture.format.is_vec(){
                        cxtexture.update_vec_texture(
                            metal_cx,
                        );
                    }
                    
                    if let Some(texture) = cxtexture.os.texture.as_ref() {
                        let () = unsafe {msg_send![
                            encoder,
                            setFragmentTexture: texture.as_id()
                            atIndex: i as u64
                        ]};
                        let () = unsafe {msg_send![
                            encoder,
                            setVertexTexture: texture.as_id()
                            atIndex: i as u64
                        ]};
                    }
                    
                }
                self.os.draw_calls_done += 1;
                if let Some(inner) = geometry.os.index_buffer.get().cpu_read().inner.as_ref() {
                    
                    let () = unsafe {msg_send![
                        encoder,
                        drawIndexedPrimitives: MTLPrimitiveType::Triangle
                        indexCount: geometry.indices.len() as u64
                        indexType: MTLIndexType::UInt32
                        indexBuffer: inner.buffer.as_id()
                        indexBufferOffset: 0
                        instanceCount: instances
                    ]};
                }
                else {crate::error!("Drawing error: index_buffer None")}
                
                gpu_read_guards.push(draw_item.os.instance_buffer.get().gpu_read());
                gpu_read_guards.push(geometry.os.vertex_buffer.get().gpu_read());
                gpu_read_guards.push(geometry.os.index_buffer.get().gpu_read());
            }
        }
    }
    
    pub fn draw_pass(
        &mut self,
        pass_id: PassId,
        metal_cx: &mut MetalCx,
        mode: DrawPassMode,
    ) {
        let draw_list_id = if let Some(draw_list_id) = self.passes[pass_id].main_draw_list_id{
            draw_list_id
        }
        else{
            crate::error!("Draw pass has no draw list!");
            return
        };
        
        let pool: ObjcId = unsafe {msg_send![class!(NSAutoreleasePool), new]};
        
        let render_pass_descriptor: ObjcId = if let DrawPassMode::MTKView(view) = mode {
            unsafe{msg_send![view, currentRenderPassDescriptor]}
        }
        else{
            unsafe {msg_send![class!(MTLRenderPassDescriptorInternal), renderPassDescriptor]}
        };
        
        let dpi_factor = self.passes[pass_id].dpi_factor.unwrap();
        
        let pass_rect = self.get_pass_rect(pass_id, if mode.is_drawable().is_some() {1.0}else {dpi_factor}).unwrap();
        
        self.passes[pass_id].set_matrix(pass_rect.pos, pass_rect.size);
        self.passes[pass_id].paint_dirty = false;

        if pass_rect.size.x <0.5 || pass_rect.size.y < 0.5 {
            return
        }
        
        self.passes[pass_id].set_dpi_factor(dpi_factor);
        
        if let DrawPassMode::MTKView(_) = mode{
            let color_attachments:ObjcId = unsafe{msg_send![render_pass_descriptor, colorAttachments]};
            let color_attachment:ObjcId = unsafe{msg_send![color_attachments, objectAtIndexedSubscript: 0]};
            let color = self.passes[pass_id].clear_color;
            unsafe {
                let () = msg_send![color_attachment, setLoadAction: MTLLoadAction::Clear];
                let () = msg_send![color_attachment, setClearColor: MTLClearColor {
                    red: color.x as f64,
                    green: color.y as f64,
                    blue: color.z as f64,
                    alpha: color.w as f64
                }];
            }
        } 
        else if let Some(drawable) = mode.is_drawable() {
            let first_texture: ObjcId = unsafe {msg_send![drawable, texture]};
            let color_attachments: ObjcId = unsafe {msg_send![render_pass_descriptor, colorAttachments]};
            let color_attachment: ObjcId = unsafe {msg_send![color_attachments, objectAtIndexedSubscript: 0]};
            
            let () = unsafe {msg_send![
                color_attachment,
                setTexture: first_texture
            ]};
            let color = self.passes[pass_id].clear_color;
            unsafe {
                let () = msg_send![color_attachment, setLoadAction: MTLLoadAction::Clear];
                let () = msg_send![color_attachment, setClearColor: MTLClearColor {
                    red: color.x as f64,
                    green: color.y as f64,
                    blue: color.z as f64,
                    alpha: color.w as f64
                }];
            }
        }
        else {
            for (index, color_texture) in self.passes[pass_id].color_textures.iter().enumerate() {

                let color_attachments: ObjcId = unsafe {msg_send![render_pass_descriptor, colorAttachments]};
                let color_attachment: ObjcId = unsafe {msg_send![color_attachments, objectAtIndexedSubscript: index as u64]};
                
                let cxtexture = &mut self.textures[color_texture.texture.texture_id()];
                let size = dpi_factor * pass_rect.size; 
                cxtexture.update_render_target(metal_cx, size.x as usize, size.y as usize);
                
                let is_initial = cxtexture.check_initial();
                
                if let Some(texture) = cxtexture.os.texture.as_ref() {
                    let () = unsafe {msg_send![
                        color_attachment,
                        setTexture: texture.as_id()
                    ]};
                }
                else {
                    crate::error!("draw_pass_to_texture invalid render target");
                }
                
                unsafe {msg_send![color_attachment, setStoreAction: MTLStoreAction::Store]}
                match color_texture.clear_color {
                    PassClearColor::InitWith(color) => {
                        if is_initial {
                            unsafe {
                                let () = msg_send![color_attachment, setLoadAction: MTLLoadAction::Clear];
                                let () = msg_send![color_attachment, setClearColor: MTLClearColor {
                                    red: color.x as f64,
                                    green: color.y as f64,
                                    blue: color.z as f64,
                                    alpha: color.w as f64
                                }];
                            }
                        }
                        else {
                            unsafe {let () = msg_send![color_attachment, setLoadAction: MTLLoadAction::Load];}
                        }
                    },
                    PassClearColor::ClearWith(color) => {
                        unsafe {
                            let () = msg_send![color_attachment, setLoadAction: MTLLoadAction::Clear];
                            let () = msg_send![color_attachment, setClearColor: MTLClearColor {
                                red: color.x as f64,
                                green: color.y as f64,
                                blue: color.z as f64,
                                alpha: color.w as f64
                            }];
                        }
                    }
                }
            }
        }
        // attach depth texture
        if let Some(depth_texture) = &self.passes[pass_id].depth_texture {
            let cxtexture = &mut self.textures[depth_texture.texture_id()];
            let size = dpi_factor * pass_rect.size;
            cxtexture.update_depth_stencil(metal_cx, size.x as usize, size.y as usize);
            let is_initial = cxtexture.check_initial();
            
            let depth_attachment: ObjcId = unsafe {msg_send![render_pass_descriptor, depthAttachment]};
            
            if let Some(texture) = cxtexture.os.texture.as_ref() {
                unsafe {msg_send![depth_attachment, setTexture: texture.as_id()]}
            }
            else {
                crate::error!("draw_pass_to_texture invalid render target");
            }
            let () = unsafe {msg_send![depth_attachment, setStoreAction: MTLStoreAction::Store]};
            
            match self.passes[pass_id].clear_depth {
                PassClearDepth::InitWith(depth) => {
                    if is_initial {
                        let () = unsafe {msg_send![depth_attachment, setLoadAction: MTLLoadAction::Clear]};
                        let () = unsafe {msg_send![depth_attachment, setClearDepth: depth as f64]};
                    }
                    else {
                        let () = unsafe {msg_send![depth_attachment, setLoadAction: MTLLoadAction::Load]};
                    }
                },
                PassClearDepth::ClearWith(depth) => {
                    let () = unsafe {msg_send![depth_attachment, setLoadAction: MTLLoadAction::Clear]};
                    let () = unsafe {msg_send![depth_attachment, setClearDepth: depth as f64]};
                }
            }
            // create depth state
            if self.passes[pass_id].os.mtl_depth_state.is_none() {
                
                let desc: ObjcId = unsafe {msg_send![class!(MTLDepthStencilDescriptor), new]};
                let () = unsafe {msg_send![desc, setDepthCompareFunction: MTLCompareFunction::LessEqual]};
                let () = unsafe {msg_send![desc, setDepthWriteEnabled: true]};
                let depth_stencil_state: ObjcId = unsafe {msg_send![metal_cx.device, newDepthStencilStateWithDescriptor: desc]};
                self.passes[pass_id].os.mtl_depth_state = Some(depth_stencil_state);
            }
        }
        
        let command_buffer: ObjcId = unsafe {msg_send![metal_cx.command_queue, commandBuffer]};
        let encoder: ObjcId = unsafe {msg_send![command_buffer, renderCommandEncoderWithDescriptor: render_pass_descriptor]};
        
        if let Some(depth_state) = self.passes[pass_id].os.mtl_depth_state {
            let () = unsafe {msg_send![encoder, setDepthStencilState: depth_state]};
        }

        let () = unsafe {msg_send![encoder, setViewport: MTLViewport {
            originX: 0.0,
            originY: 0.0,
            width: dpi_factor * pass_rect.size.x,
            height: dpi_factor * pass_rect.size.y,
            znear: 0.0,
            zfar: 1.0,
        }]};
        
        let mut zbias = 0.0;
        let zbias_step = self.passes[pass_id].zbias_step;
        let mut gpu_read_guards = Vec::new();
        
        self.render_view(
            pass_id,
            draw_list_id,
            &mut zbias,
            zbias_step,
            encoder,
            command_buffer,
            &mut gpu_read_guards,
            &metal_cx,
        );
        
        let () = unsafe {msg_send![encoder, endEncoding]};
        
        match mode {
            DrawPassMode::MTKView(view)=>{
                let drawable:ObjcId = unsafe {msg_send![view, currentDrawable]};
                let () = unsafe {msg_send![command_buffer, presentDrawable: drawable]};
                
                self.commit_command_buffer(None, command_buffer, gpu_read_guards);
            }
            DrawPassMode::Texture => {
                self.commit_command_buffer(None, command_buffer, gpu_read_guards);
            }
            DrawPassMode::StdinMain(stdin_frame) => {
                self.commit_command_buffer(Some(stdin_frame), command_buffer, gpu_read_guards);
            }
            DrawPassMode::Drawable(drawable) => {
                let () = unsafe {msg_send![command_buffer, presentDrawable: drawable]};
                self.commit_command_buffer(None, command_buffer, gpu_read_guards);
            }
            DrawPassMode::Resizing(drawable) => {
                self.commit_command_buffer(None, command_buffer, gpu_read_guards);
                let () = unsafe {msg_send![command_buffer, waitUntilScheduled]};
                let () = unsafe {msg_send![drawable, present]};
            }
        }
        let () = unsafe {msg_send![pool, release]};
    }
    
    fn commit_command_buffer(&mut self, stdin_frame: Option<PresentableDraw>, command_buffer: ObjcId, gpu_read_guards: Vec<MetalRwLockGpuReadGuard>) {
        let gpu_read_guards = Mutex::new(Some(gpu_read_guards));
        //let present_index = Arc::clone(&self.os.present_index);
        //Self::stdin_send_draw_complete(&present_index);
        let start_time = self.os.start_time.unwrap();
        let () = unsafe {msg_send![
            command_buffer,
            addCompletedHandler: &objc_block!(move | command_buffer: ObjcId | {
                let start:f64 = unsafe {msg_send![command_buffer, GPUStartTime]};
                let end:f64 = unsafe {msg_send![command_buffer, GPUEndTime]};
                if let Some(_stdin_frame) = stdin_frame {
                    #[cfg(target_os = "macos")]
                    Self::stdin_send_draw_complete(_stdin_frame);
                }
                // lets send off our gpu time
                let duration = end - start;
                let start = Instant::now().duration_since(start_time).as_secs_f64() - duration;
                let end = start + duration;
                Cx::send_studio_message(AppToStudio::GPUSample(GPUSample{
                    start, end
                }));
                
                drop(gpu_read_guards.lock().unwrap().take().unwrap());
            })
        ]};
        let () = unsafe {msg_send![command_buffer, commit]};
    } 
    
    pub (crate) fn mtl_compile_shaders(&mut self, metal_cx: &MetalCx) {
        for draw_shader_ptr in &self.draw_shaders.compile_set {
            if let Some(item) = self.draw_shaders.ptr_to_item.get(&draw_shader_ptr) {
                let cx_shader = &mut self.draw_shaders.shaders[item.draw_shader_id];
                let draw_shader_def = self.shader_registry.draw_shader_defs.get(&draw_shader_ptr);
                let gen = generate_metal::generate_shader(
                    draw_shader_def.as_ref().unwrap(),
                    &cx_shader.mapping.const_table,
                    &self.shader_registry
                );
                
                if cx_shader.mapping.flags.debug {
                    crate::log!("{}", gen.mtlsl);
                }
                // lets see if we have the shader already
                for (index, ds) in self.draw_shaders.os_shaders.iter().enumerate() {
                    if ds.mtlsl == gen.mtlsl {
                        cx_shader.os_shader_id = Some(index);
                        break;
                    }
                }
                if cx_shader.os_shader_id.is_none() {
                    if let Some(shp) = CxOsDrawShader::new(metal_cx, gen) {
                        cx_shader.os_shader_id = Some(self.draw_shaders.os_shaders.len());
                        self.draw_shaders.os_shaders.push(shp);
                    }
                }
            }
        }
        self.draw_shaders.compile_set.clear();
    }
    
    #[cfg(target_os="macos")]
    pub fn share_texture_for_presentable_image(
        &mut self,
        texture: &Texture,
    ) -> crate::cx_stdin::SharedPresentableImageOsHandle {
        let cxtexture = &mut self.textures[texture.texture_id()];
        cxtexture.update_shared_texture(self.os.metal_device.unwrap());

        // HACK(eddyb) macOS has no real `SharedPresentableImageOsHandle` because
        // the texture is actually shared through an XPC helper service instead,
        // based entirely on its `PresentableImageId`.
        crate::cx_stdin::SharedPresentableImageOsHandle {
            _dummy_for_macos: None,
        }
    }
    
    #[cfg(any(target_os="ios", target_os="tvos"))]
    pub fn share_texture_for_presentable_image(
        &mut self,
        _texture: &Texture,
    ) -> crate::cx_stdin::SharedPresentableImageOsHandle {
        crate::cx_stdin::SharedPresentableImageOsHandle {
            _dummy_for_unsupported: None,
        }
    }
}

pub enum DrawPassMode {
    Texture,
    MTKView(ObjcId),
    StdinMain(PresentableDraw),
    Drawable(ObjcId),
    Resizing(ObjcId)
}

impl DrawPassMode {
    fn is_drawable(&self) -> Option<ObjcId> {
        match self {
            Self::Drawable(obj) | Self::Resizing(obj) => Some(*obj),
            Self::StdinMain(_) | Self::Texture | Self::MTKView(_) => None
        }
    }
}

pub struct MetalCx {
    pub device: ObjcId,
    command_queue: ObjcId
}


#[derive(Clone, Default)]
pub struct CxOsView {
}

#[derive(Default, Clone)]
pub struct CxOsPass {
    mtl_depth_state: Option<ObjcId>
}

pub enum PackType {
    Packed,
    Unpacked
}

pub struct SlErr {
    _msg: String
}

impl MetalCx {
    
    pub (crate) fn new() -> MetalCx {
        let device = get_default_metal_device().expect("Cannot get default metal device");
        MetalCx {
            command_queue: unsafe {msg_send![device, newCommandQueue]},
            device: device
        }
    }
}

/**************************************************************************************************/

pub struct CxOsDrawShader {
    _library: RcObjcId,
    render_pipeline_state: RcObjcId,
    draw_uniform_buffer_id: Option<u64>,
    pass_uniform_buffer_id: Option<u64>,
    view_uniform_buffer_id: Option<u64>,
    user_uniform_buffer_id: Option<u64>,
    mtlsl: String,
}

impl CxOsDrawShader {
    pub (crate) fn new(
        metal_cx: &MetalCx,
        shader: MetalGeneratedShader,
    ) -> Option<Self> {
        let options = RcObjcId::from_owned(unsafe {msg_send![class!(MTLCompileOptions), new]});
        unsafe {
            let _: () = msg_send![options.as_id(), setFastMathEnabled: YES];
        };
        
        let mut error: ObjcId = nil;
        
        let library = RcObjcId::from_owned(match NonNull::new(unsafe {
            msg_send![
                metal_cx.device,
                newLibraryWithSource: str_to_nsstring(&shader.mtlsl)
                options: options
                error: &mut error
            ]
        }) {
            Some(library) => library,
            None => {
                let description: ObjcId = unsafe {msg_send![error, localizedDescription]};
                let string = nsstring_to_string(description);
                let mut out = format!("{}\n", string);
                for (index, line) in shader.mtlsl.split("\n").enumerate() {
                    out.push_str(&format!("{}: {}\n", index + 1, line));
                }
                crate::error!("{}", out);
                panic!("{}", string);
            }
        });
        
        let descriptor = RcObjcId::from_owned(NonNull::new(unsafe {
            msg_send![class!(MTLRenderPipelineDescriptor), new]
        }).unwrap());
        
        let vertex_function = RcObjcId::from_owned(NonNull::new(unsafe {
            msg_send![library.as_id(), newFunctionWithName: str_to_nsstring("vertex_main")]
        }).unwrap());
        
        let fragment_function = RcObjcId::from_owned(NonNull::new(unsafe {
            msg_send![library.as_id(), newFunctionWithName: str_to_nsstring("fragment_main")]
        }).unwrap());
        
        let render_pipeline_state = RcObjcId::from_owned(NonNull::new(unsafe {
            let _: () = msg_send![descriptor.as_id(), setVertexFunction: vertex_function];
            let _: () = msg_send![descriptor.as_id(), setFragmentFunction: fragment_function];
            
            let color_attachments: ObjcId = msg_send![descriptor.as_id(), colorAttachments];
            let color_attachment: ObjcId = msg_send![color_attachments, objectAtIndexedSubscript: 0];
            let () = msg_send![color_attachment, setPixelFormat: MTLPixelFormat::BGRA8Unorm];
            let () = msg_send![color_attachment, setBlendingEnabled: YES];
            let () = msg_send![color_attachment, setRgbBlendOperation: MTLBlendOperation::Add];
            let () = msg_send![color_attachment, setAlphaBlendOperation: MTLBlendOperation::Add];
            let () = msg_send![color_attachment, setSourceRGBBlendFactor: MTLBlendFactor::One];
            let () = msg_send![color_attachment, setSourceAlphaBlendFactor: MTLBlendFactor::One];
            let () = msg_send![color_attachment, setDestinationRGBBlendFactor: MTLBlendFactor::OneMinusSourceAlpha];
            let () = msg_send![color_attachment, setDestinationAlphaBlendFactor: MTLBlendFactor::OneMinusSourceAlpha];
            
            let () = msg_send![descriptor.as_id(), setDepthAttachmentPixelFormat: MTLPixelFormat::Depth32Float];
            
            let mut error: ObjcId = nil;
            msg_send![
                metal_cx.device,
                newRenderPipelineStateWithDescriptor: descriptor
                error: &mut error
            ]
        }).unwrap());
        
        let mut draw_uniform_buffer_id = None;
        let mut pass_uniform_buffer_id = None;
        let mut view_uniform_buffer_id = None;
        let mut user_uniform_buffer_id = None;
        
        let mut buffer_id = 4;
        for (field, _) in shader.fields_as_uniform_blocks {
            match field.0 {
                live_id!(draw) => draw_uniform_buffer_id = Some(buffer_id),
                live_id!(pass) => pass_uniform_buffer_id = Some(buffer_id),
                live_id!(view) => view_uniform_buffer_id = Some(buffer_id),
                live_id!(user) => user_uniform_buffer_id = Some(buffer_id),
                _ => panic!()
            }
            buffer_id += 1;
        }
        
        return Some(Self {
            _library: library,
            render_pipeline_state,
            draw_uniform_buffer_id,
            pass_uniform_buffer_id,
            view_uniform_buffer_id,
            user_uniform_buffer_id,
            mtlsl: shader.mtlsl
        });
    }
}

#[derive(Default)]
pub struct CxOsDrawCall {
    //pub uni_dr: MetalBuffer,
    instance_buffer: MetalBufferQueue,
}

#[derive(Default)]
pub struct CxOsGeometry {
    vertex_buffer: MetalBufferQueue,
    index_buffer: MetalBufferQueue,
}

#[derive(Default)]
struct MetalBufferQueue {
    queue: [MetalRwLock<MetalBuffer>; 3],
    index: usize,
}

impl MetalBufferQueue {
    fn get(&self) -> &MetalRwLock<MetalBuffer> {
        &self.queue[self.index]
    }
    
    fn get_mut(&mut self) -> &mut MetalRwLock<MetalBuffer> {
        &mut self.queue[self.index]
    }
    
    fn next(&mut self) {
        self.index = (self.index + 1) % self.queue.len();
    }
}

#[derive(Default)]
struct MetalBuffer {
    inner: Option<MetalBufferInner>,
}

impl MetalBuffer {
    fn update<T>(&mut self, metal_cx: &MetalCx, data: &[T]) where T: std::fmt::Debug {
        let len = data.len() * std::mem::size_of::<T>();
        if len == 0 {
            self.inner = None;
            return;
        }
        if self.inner.as_ref().map_or(0, | inner | inner.len) < len {
            self.inner = Some(MetalBufferInner {
                len,
                buffer: RcObjcId::from_owned(NonNull::new(unsafe {
                    msg_send![
                        metal_cx.device,
                        newBufferWithLength: len as u64
                        options: nil
                    ]
                }).unwrap())
            });
        }
        let inner = self.inner.as_ref().unwrap();
        unsafe {
            let contents: *mut u8 = msg_send![inner.buffer.as_id(), contents];
            
            //println!("Buffer write {} buf {} data {:?}", command_buffer as *const _ as u64, inner.buffer.as_id() as *const _ as u64, data);
            
            std::ptr::copy(data.as_ptr() as *const u8, contents, len);
            /*
            let _: () = msg_send![
                inner.buffer.as_id(),
                didModifyRange: NSRange {
                    location: 0,
                    length: len as u64
                }
            ];*/
        }
    }
}

struct MetalBufferInner {
    len: usize,
    buffer: RcObjcId,
}

#[derive(Default)]
pub struct CxOsTexture {
    texture: Option<RcObjcId>
}
fn texture_pixel_to_mtl_pixel(pix:&TexturePixel)-> MTLPixelFormat {
     match pix{
         TexturePixel::BGRAu8 => MTLPixelFormat::BGRA8Unorm,
         TexturePixel::RGBAf16 => MTLPixelFormat::RGBA16Float,
         TexturePixel::RGBAf32 => MTLPixelFormat::RGBA32Float,
         TexturePixel::Ru8  => MTLPixelFormat::R8Unorm,
         TexturePixel::RGu8  => MTLPixelFormat::RG8Unorm,
         TexturePixel::Rf32  => MTLPixelFormat::R32Float,
         TexturePixel::D32 => MTLPixelFormat::Depth32Float,
     }   
}
impl CxTexture {
    
    fn update_vec_texture(
        &mut self,
        metal_cx: &MetalCx,
    ) {
        if self.alloc_vec() {
            let alloc = self.alloc.as_ref().unwrap();
            
            let descriptor = RcObjcId::from_owned(NonNull::new(unsafe {
                msg_send![class!(MTLTextureDescriptor), new]
            }).unwrap());
                        
            let _: () = unsafe {msg_send![descriptor.as_id(), setTextureType: MTLTextureType::D2]};
            let _: () = unsafe {msg_send![descriptor.as_id(), setDepth: 1u64]};
            let _: () = unsafe {msg_send![descriptor.as_id(), setStorageMode: MTLStorageMode::Shared]};
            let _: () = unsafe {msg_send![descriptor.as_id(), setUsage: MTLTextureUsage::ShaderRead]};
            let _: () = unsafe {msg_send![descriptor.as_id(), setWidth: alloc.width as u64]};
            let _: () = unsafe {msg_send![descriptor.as_id(), setHeight: alloc.height as u64]};
            let _: () = unsafe{msg_send![descriptor.as_id(), setPixelFormat: texture_pixel_to_mtl_pixel(&alloc.pixel)]};
            let texture:ObjcId = unsafe{msg_send![metal_cx.device, newTextureWithDescriptor: descriptor]};
            self.os.texture = Some(RcObjcId::from_owned(NonNull::new(texture).unwrap()));
        }
        if self.check_updated(){
            fn update_data(texture:&Option<RcObjcId>, width: usize, height: usize, bpp: u64, data: *const std::ffi::c_void){
                let region = MTLRegion {
                    origin: MTLOrigin {x: 0, y: 0, z: 0},
                    size: MTLSize {width: width as u64, height: height as u64, depth: 1}
                };
                                            
                let () = unsafe {msg_send![
                    texture.as_ref().unwrap().as_id(),
                    replaceRegion: region
                    mipmapLevel: 0
                    withBytes: data
                    bytesPerRow: (width as u64) * bpp
                ]};
            }
            
            match &self.format{
                TextureFormat::VecBGRAu8_32{width, height, data}=>{
                    update_data(&self.os.texture, *width, *height, 4,  data.as_ptr() as *const std::ffi::c_void);
                }
                TextureFormat::VecRGBAf32{width, height, data}=>{
                    update_data(&self.os.texture, *width, *height, 16,  data.as_ptr() as *const std::ffi::c_void);
                }
                TextureFormat::VecRu8{width, height, data, ..}=>{
                    update_data(&self.os.texture, *width, *height, 1,  data.as_ptr() as *const std::ffi::c_void);
                }
                TextureFormat::VecRGu8{width, height, data, ..}=>{
                    update_data(&self.os.texture, *width, *height, 2,  data.as_ptr() as *const std::ffi::c_void);
                }
                TextureFormat::VecRf32{width, height, data}=>{
                    update_data(&self.os.texture, *width, *height, 4,  data.as_ptr() as *const std::ffi::c_void);
                }
                _=>panic!()
            }
        }
    }
    
    #[cfg(target_os = "macos")]
    fn update_shared_texture(
        &mut self,
        metal_device: ObjcId,
    ) {
        // we need a width/height for this one.
        if !self.alloc_shared(){
            return
        }
        let alloc = self.alloc.as_ref().unwrap();
        let descriptor = RcObjcId::from_owned(NonNull::new(unsafe {
            msg_send![class!(MTLTextureDescriptor), new]
        }).unwrap());
            
        let _: () = unsafe{msg_send![descriptor.as_id(), setTextureType: MTLTextureType::D2]};
        let _: () = unsafe{msg_send![descriptor.as_id(), setWidth: alloc.width as u64]};
        let _: () = unsafe{msg_send![descriptor.as_id(), setHeight: alloc.height as u64]};
        let _: () = unsafe{msg_send![descriptor.as_id(), setDepth: 1u64]};
        let _: () = unsafe{msg_send![descriptor.as_id(), setStorageMode: MTLStorageMode::Private]};
        let _: () = unsafe{msg_send![descriptor.as_id(), setUsage: MTLTextureUsage::RenderTarget]};
        let _: () = unsafe{msg_send![descriptor.as_id(), setPixelFormat: texture_pixel_to_mtl_pixel(&alloc.pixel)]};
        match &self.format {
            TextureFormat::SharedBGRAu8{id, ..} => {
                let texture: ObjcId = unsafe{msg_send![metal_device, newSharedTextureWithDescriptor: descriptor]};
                let shared: ObjcId = unsafe{msg_send![texture, newSharedTextureHandle]};
                store_xpc_service_texture(*id, shared);
                let _: () = unsafe{msg_send![shared, release]};
                self.os.texture = Some(RcObjcId::from_owned(NonNull::new(texture).unwrap()));
            }
            _ => panic!(),
        }
    }
    
    #[cfg(target_os = "macos")]
    pub fn update_from_shared_handle(
        &mut self,
        metal_cx: &MetalCx,
        shared_handle: ObjcId,
    ) -> bool {
        // we need a width/height for this one.
        if !self.alloc_shared(){
            return true
        }
        let alloc = self.alloc.as_ref().unwrap();
    
        let texture = RcObjcId::from_owned(NonNull::new(unsafe {
            msg_send![metal_cx.device, newSharedTextureWithHandle: shared_handle]
        }).unwrap());
        let width: u64 = unsafe{msg_send![texture.as_id(), width]};
        let height: u64 = unsafe{msg_send![texture.as_id(), height]};
        // FIXME(eddyb) can these be an assert now?
        if width != alloc.width as u64|| height != alloc.height as u64{
            return false
        }
        self.os.texture = Some(texture);
        true
    }
    
    fn update_render_target(
        &mut self,
        metal_cx: &MetalCx,
        width: usize,
        height: usize
    ) {
        if self.alloc_render(width, height){
            let alloc = self.alloc.as_ref().unwrap();
            let descriptor = RcObjcId::from_owned(NonNull::new(unsafe {
                msg_send![class!(MTLTextureDescriptor), new]
            }).unwrap());
            
            let _: () = unsafe{msg_send![descriptor.as_id(), setTextureType: MTLTextureType::D2]};
            let _: () = unsafe{msg_send![descriptor.as_id(), setWidth: alloc.width as u64]};
            let _: () = unsafe{msg_send![descriptor.as_id(), setHeight: alloc.height as u64]};
            let _: () = unsafe{msg_send![descriptor.as_id(), setDepth: 1u64]};
            let _: () = unsafe{msg_send![descriptor.as_id(), setStorageMode: MTLStorageMode::Private]};
            let _: () = unsafe{msg_send![descriptor.as_id(), setUsage: MTLTextureUsage::RenderTarget]};
            let _: () = unsafe{msg_send![descriptor.as_id(),setPixelFormat: texture_pixel_to_mtl_pixel(&alloc.pixel)]};
            let texture = RcObjcId::from_owned(NonNull::new(unsafe {
                msg_send![metal_cx.device, newTextureWithDescriptor: descriptor]
            }).unwrap());
            
            self.os.texture = Some(texture); 
        }
    }
    
    
    fn update_depth_stencil(
        &mut self,
        metal_cx: &MetalCx,
        width: usize,
        height: usize
    ) {
        if self.alloc_depth(width, height){
       
            let alloc = self.alloc.as_ref().unwrap();
            let descriptor = RcObjcId::from_owned(NonNull::new(unsafe {
                msg_send![class!(MTLTextureDescriptor), new]
            }).unwrap());
                        
            let _: () = unsafe{msg_send![descriptor.as_id(), setTextureType: MTLTextureType::D2]};
            let _: () = unsafe{msg_send![descriptor.as_id(), setWidth: alloc.width as u64]};
            let _: () = unsafe{msg_send![descriptor.as_id(), setHeight: alloc.height as u64]};
            let _: () = unsafe{msg_send![descriptor.as_id(), setDepth: 1u64]};
            let _: () = unsafe{msg_send![descriptor.as_id(), setStorageMode: MTLStorageMode::Private]};
            let _: () = unsafe{msg_send![descriptor.as_id(), setUsage: MTLTextureUsage::RenderTarget]};
            let _: () = unsafe{msg_send![
                descriptor.as_id(),
                setPixelFormat: texture_pixel_to_mtl_pixel(&alloc.pixel)
            ]};
            let texture = RcObjcId::from_owned(NonNull::new(unsafe {
                msg_send![metal_cx.device, newTextureWithDescriptor: descriptor]
            }).unwrap());
            self.os.texture = Some(texture);
        }
    }    
}

#[derive(Default)]
struct MetalRwLock<T> {
    inner: Arc<MetalRwLockInner>,
    value: T
}

impl<T> MetalRwLock<T> {
    fn cpu_read(&self) -> &T {
        &self.value
    }
    
    fn gpu_read(&self) -> MetalRwLockGpuReadGuard {
        let mut reader_count = self.inner.reader_count.lock().unwrap();
        *reader_count += 1;
        MetalRwLockGpuReadGuard {
            inner: self.inner.clone()
        }
    }
    
    fn cpu_write(&mut self) -> &mut T {
        let mut reader_count = self.inner.reader_count.lock().unwrap();
        while *reader_count != 0 {
            reader_count = self.inner.condvar.wait(reader_count).unwrap();
        }
        &mut self.value
    }
}

#[derive(Default)]
struct MetalRwLockInner {
    reader_count: Mutex<usize>,
    condvar: Condvar,
}

struct MetalRwLockGpuReadGuard {
    inner: Arc<MetalRwLockInner>
}

impl Drop for MetalRwLockGpuReadGuard {
    fn drop(&mut self) {
        let mut reader_count = self.inner.reader_count.lock().unwrap();
        *reader_count -= 1;
        if *reader_count == 0 {
            self.inner.condvar.notify_one();
        }
    }
}

pub fn get_default_metal_device() -> Option<ObjcId> {
    unsafe {
        let dev = MTLCreateSystemDefaultDevice();
        if dev == nil {None} else {Some(dev)}
    }
}

pub fn get_all_metal_devices() -> Vec<ObjcId> {
    #[cfg(any(target_os = "ios", target_os = "tvos"))]
    unsafe {
        vec![MTLCreateSystemDefaultDevice()]
    }
    #[cfg(target_os = "macos")]
    unsafe {
        let array = MTLCopyAllDevices();
        let count: u64 = msg_send![array, count];
        let ret = (0..count)
            .map( | i | msg_send![array, objectAtIndex: i])
        // The elements of this array are references---we convert them to owned references
        // (which just means that we increment the reference count here, and it is
        // decremented in the `Drop` impl for `Device`)
            .map( | device: *mut Object | msg_send![device, retain])
            .collect();
        let () = msg_send![array, release];
        ret
    }
}


