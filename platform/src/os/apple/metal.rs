use {
    makepad_objc_sys::{
        msg_send,
        runtime::{YES, NO},
        sel,
        class,
        sel_impl,
    },
    crate::{
        makepad_shader_compiler::{
            generate_metal,
            generate_metal::MetalGeneratedShader,
        },
        makepad_math::*,
        makepad_live_id::*,
        makepad_error_log::*,
        os::{
            apple::frameworks::*,
            apple::apple_util::{
                nsstring_to_string,
                str_to_nsstring,
            },
            metal_xpc::store_xpc_service_texture,
            cocoa_app::CocoaApp,
            cocoa_window::CocoaWindow,
        },
        draw_list::DrawListId,
        event::WindowGeom,
        cx::Cx,
        pass::{PassClearColor, PassClearDepth, PassId},
        window::WindowId,
        texture::{
            TextureFormat,
            TextureDesc,
        },
    },
    std::{
        sync::{
            Arc,
            Condvar,
            Mutex,
        }
    }
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
                if sh.platform.is_none() { // shader didnt compile somehow
                    continue;
                }
                let shp = &self.draw_shaders.platform[sh.platform.unwrap()];
                
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
                else {error!("Drawing error: vertex_buffer None")}
                
                if let Some(inner) = draw_item.os.instance_buffer.get().cpu_read().inner.as_ref() {
                    unsafe {msg_send![
                        encoder,
                        setVertexBuffer: inner.buffer.as_id()
                        offset: 0
                        atIndex: 1
                    ]}
                }
                else {error!("Drawing error: instance_buffer None")}
                
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
                    
                    let texture_id = if let Some(texture_id) = draw_call.texture_slots[i] {
                        texture_id
                    }else {
                        continue;
                    };
                    
                    let cxtexture = &mut self.textures[texture_id];
                    
                    if cxtexture.desc.format.is_shared() {
                        cxtexture.os.update_shared_texture(
                            metal_cx,
                            &cxtexture.desc,
                        );
                    }
                    else if cxtexture.update_image {
                        cxtexture.update_image = false;
                        cxtexture.os.update_normal_texture(
                            metal_cx,
                            &cxtexture.desc,
                            &cxtexture.image_u32
                        );
                    }
                    
                    if let Some(inner) = cxtexture.os.inner.as_ref() {
                        let () = unsafe {msg_send![
                            encoder,
                            setFragmentTexture: inner.texture.as_id()
                            atIndex: i as u64
                        ]};
                        let () = unsafe {msg_send![
                            encoder,
                            setVertexTexture: inner.texture.as_id()
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
                else {error!("Drawing error: index_buffer None")}
                
                gpu_read_guards.push(draw_item.os.instance_buffer.get().gpu_read());
                gpu_read_guards.push(geometry.os.vertex_buffer.get().gpu_read());
                gpu_read_guards.push(geometry.os.index_buffer.get().gpu_read());
            }
        }
    }
    
    pub fn draw_pass(
        &mut self,
        pass_id: PassId,
        dpi_factor: f64,
        metal_cx: &mut MetalCx,
        mode: DrawPassMode,
    ) {
        let draw_list_id = self.passes[pass_id].main_draw_list_id.unwrap();
        
        let pool: ObjcId = unsafe {msg_send![class!(NSAutoreleasePool), new]};
        
        let render_pass_descriptor: ObjcId = unsafe {msg_send![class!(MTLRenderPassDescriptorInternal), renderPassDescriptor]};
        
        let pass_size = self.passes[pass_id].pass_size;
        
        self.passes[pass_id].set_matrix(DVec2::default(), pass_size);
        self.passes[pass_id].paint_dirty = false;
        
        let dpi_factor = if let Some(override_dpi_factor) = self.passes[pass_id].override_dpi_factor {
            override_dpi_factor
        }
        else {
            dpi_factor
        };
        
        self.passes[pass_id].set_dpi_factor(dpi_factor);
        
        if let Some(drawable) = mode.is_drawable() {
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
                
                let cxtexture = &mut self.textures[color_texture.texture_id];
                
                cxtexture.os.update_render_target(metal_cx, AttachmentKind::Color, &cxtexture.desc, dpi_factor * pass_size);
                
                let is_initial = cxtexture.os.inner.as_mut().unwrap().initial();
                
                if let Some(inner) = cxtexture.os.inner.as_ref() {
                    let () = unsafe {msg_send![
                        color_attachment,
                        setTexture: inner.texture.as_id()
                    ]};
                }
                else {
                    error!("draw_pass_to_texture invalid render target");
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
        if let Some(depth_texture_id) = self.passes[pass_id].depth_texture {
            let cxtexture = &mut self.textures[depth_texture_id];
            cxtexture.os.update_render_target(metal_cx, AttachmentKind::Depth, &cxtexture.desc, dpi_factor * pass_size);
            let is_initial = cxtexture.os.inner.as_mut().unwrap().initial();
            
            let depth_attachment: ObjcId = unsafe {msg_send![render_pass_descriptor, depthAttachment]};
            
            if let Some(inner) = cxtexture.os.inner.as_ref() {
                unsafe {msg_send![depth_attachment, setTexture: inner.texture.as_id()]}
            }
            else {
                error!("draw_pass_to_texture invalid render target");
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
            if self.passes[pass_id].platform.mtl_depth_state.is_none() {
                
                let desc: ObjcId = unsafe {msg_send![class!(MTLDepthStencilDescriptor), new]};
                let () = unsafe {msg_send![desc, setDepthCompareFunction: MTLCompareFunction::LessEqual]};
                let () = unsafe {msg_send![desc, setDepthWriteEnabled: true]};
                let depth_stencil_state: ObjcId = unsafe {msg_send![metal_cx.device, newDepthStencilStateWithDescriptor: desc]};
                self.passes[pass_id].platform.mtl_depth_state = Some(depth_stencil_state);
            }
        }
        
        let command_buffer: ObjcId = unsafe {msg_send![metal_cx.command_queue, commandBuffer]};
        let encoder: ObjcId = unsafe {msg_send![command_buffer, renderCommandEncoderWithDescriptor: render_pass_descriptor]};
        
        if let Some(depth_state) = self.passes[pass_id].platform.mtl_depth_state {
            let () = unsafe {msg_send![encoder, setDepthStencilState: depth_state]};
        }
        
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
            DrawPassMode::Texture => {
                self.commit_command_buffer(None, command_buffer, gpu_read_guards);
            }
            DrawPassMode::StdinMain => {
                self.commit_command_buffer(Some(0), command_buffer, gpu_read_guards);
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
    
    fn commit_command_buffer(&mut self, _stdin_frame:Option<u32>, command_buffer: ObjcId, gpu_read_guards: Vec<MetalRwLockGpuReadGuard>) {
        let gpu_read_guards = Mutex::new(Some(gpu_read_guards));
        let () = unsafe {msg_send![
            command_buffer,
            addCompletedHandler: &objc_block!(move | _command_buffer: ObjcId | {
                drop(gpu_read_guards.lock().unwrap().take().unwrap());
            })
        ]};
        let () = unsafe {msg_send![command_buffer, commit]};
    }
}

pub enum DrawPassMode {
    Texture,
    StdinMain,
    Drawable(ObjcId),
    Resizing(ObjcId)
}

impl DrawPassMode {
    fn is_drawable(&self) -> Option<ObjcId> {
        match self {
            Self::Drawable(obj) | Self::Resizing(obj) => Some(*obj),
            Self::StdinMain | Self::Texture => None
        }
    }
}

pub struct MetalCx {
    device: ObjcId,
    command_queue: ObjcId
}


#[derive(Clone)]
pub struct MetalWindow {
    pub window_id: WindowId,
    pub window_geom: WindowGeom,
    cal_size: DVec2,
    pub ca_layer: ObjcId,
    pub cocoa_window: Box<CocoaWindow>,
    pub is_resizing: bool
}

impl MetalWindow {
    pub (crate) fn new(
        window_id: WindowId,
        metal_cx: &MetalCx,
        cocoa_app: &mut CocoaApp,
        inner_size: DVec2,
        position: Option<DVec2>,
        title: &str
    ) -> MetalWindow {
        
        let ca_layer: ObjcId = unsafe {msg_send![class!(CAMetalLayer), new]};
        
        let mut cocoa_window = Box::new(CocoaWindow::new(cocoa_app, window_id));
        
        cocoa_window.init(title, inner_size, position);
        unsafe {
            let () = msg_send![ca_layer, setDevice: metal_cx.device];
            let () = msg_send![ca_layer, setPixelFormat: MTLPixelFormat::BGRA8Unorm];
            let () = msg_send![ca_layer, setPresentsWithTransaction: NO];
            let () = msg_send![ca_layer, setMaximumDrawableCount: 3];
            let () = msg_send![ca_layer, setDisplaySyncEnabled: YES];
            let () = msg_send![ca_layer, setNeedsDisplayOnBoundsChange: YES];
            let () = msg_send![ca_layer, setAutoresizingMask: (1 << 4) | (1 << 1)];
            let () = msg_send![ca_layer, setAllowsNextDrawableTimeout: NO];
            let () = msg_send![ca_layer, setDelegate: cocoa_window.view];
            let () = msg_send![ca_layer, setBackgroundColor: CGColorCreateGenericRGB(0.0, 0.0, 0.0, 1.0)];
            
            let view = cocoa_window.view;
            let () = msg_send![view, setWantsBestResolutionOpenGLSurface: YES];
            let () = msg_send![view, setWantsLayer: YES];
            let () = msg_send![view, setLayerContentsPlacement: 11];
            let () = msg_send![view, setLayer: ca_layer];
        }
        
        MetalWindow {
            is_resizing: false,
            window_id,
            cal_size: DVec2::default(),
            ca_layer,
            window_geom: cocoa_window.get_window_geom(),
            cocoa_window
        }
    }
    
    pub (crate) fn start_resize(&mut self) {
        self.is_resizing = true;
        let () = unsafe {msg_send![self.ca_layer, setPresentsWithTransaction: YES]};
    }
    
    pub (crate) fn stop_resize(&mut self) {
        self.is_resizing = false;
        let () = unsafe {msg_send![self.ca_layer, setPresentsWithTransaction: NO]};
    }
    
    pub (crate) fn resize_core_animation_layer(&mut self, _metal_cx: &MetalCx) -> bool {
        let cal_size = DVec2 {
            x: self.window_geom.inner_size.x * self.window_geom.dpi_factor,
            y: self.window_geom.inner_size.y * self.window_geom.dpi_factor
        };
        if self.cal_size != cal_size {
            self.cal_size = cal_size;
            unsafe {
                let () = msg_send![self.ca_layer, setDrawableSize: CGSize {width: cal_size.x, height: cal_size.y}];
                let () = msg_send![self.ca_layer, setContentsScale: self.window_geom.dpi_factor];
            }
            true
        }
        else {
            false
        }
    }
    
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

impl Cx {
    
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
                    log!("{}", gen.mtlsl);
                }
                // lets see if we have the shader already
                for (index, ds) in self.draw_shaders.platform.iter().enumerate() {
                    if ds.mtlsl == gen.mtlsl {
                        cx_shader.platform = Some(index);
                        break;
                    }
                }
                if cx_shader.platform.is_none() {
                    if let Some(shp) = CxOsDrawShader::new(metal_cx, gen) {
                        cx_shader.platform = Some(self.draw_shaders.platform.len());
                        self.draw_shaders.platform.push(shp);
                    }
                }
            }
        }
        self.draw_shaders.compile_set.clear();
    }
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
                error!("{}", out);
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
            
            let () = msg_send![descriptor.as_id(), setDepthAttachmentPixelFormat: MTLPixelFormat::Depth32Float_Stencil8];
            
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
            let _: () = msg_send![
                inner.buffer.as_id(),
                didModifyRange: NSRange {
                    location: 0,
                    length: len as u64
                }
            ];
        }
    }
}

struct MetalBufferInner {
    len: usize,
    buffer: RcObjcId,
}

#[derive(Default)]
pub struct CxOsTexture {
    inner: Option<CxOsTextureInner>
}

impl CxOsTexture {
    
    
    fn update_normal_texture(
        &mut self,
        metal_cx: &MetalCx,
        desc: &TextureDesc,
        data: &[u32],
    ) {
        // we need a width/height for this one.
        if desc.width.is_none() || desc.height.is_none() {
            log!("Normal texture width/height is undefined, cannot allocate it");
            return
        }
        
        let width = desc.width.unwrap() as u64;
        let height = desc.height.unwrap() as u64;
        
        match desc.format {
            TextureFormat::ImageBGRA | TextureFormat::Default => {
                if (width * height)as usize != data.len() {
                    if data.len() != 0 {
                        error!("Texture buffer not correct size {}*{} != {}", width, height, data.len());
                    }
                    return
                }
            }
            _ => panic!(),
        }
        
        let need_alloc = if let Some(inner) = &self.inner {
            CxOsTextureInner::need_alloc(width, height, desc, inner)
        }
        else {
            true
        };
        
        if need_alloc {
            let descriptor = RcObjcId::from_owned(NonNull::new(unsafe {
                msg_send![class!(MTLTextureDescriptor), new]
            }).unwrap());
            
            let texture = RcObjcId::from_owned(NonNull::new(unsafe {
                let _: () = msg_send![descriptor.as_id(), setTextureType: MTLTextureType::D2];
                let _: () = msg_send![descriptor.as_id(), setWidth: width as u64];
                let _: () = msg_send![descriptor.as_id(), setHeight: height as u64];
                let _: () = msg_send![descriptor.as_id(), setDepth: 1u64];
                let _: () = msg_send![descriptor.as_id(), setStorageMode: MTLStorageMode::Managed];
                let _: () = msg_send![descriptor.as_id(), setUsage: MTLTextureUsage::ShaderRead];
                match desc.format {
                    TextureFormat::ImageBGRA | TextureFormat::Default => {
                        let _: () = msg_send![
                            descriptor.as_id(),
                            setPixelFormat: MTLPixelFormat::BGRA8Unorm
                        ];
                        msg_send![metal_cx.device, newTextureWithDescriptor: descriptor]
                    }
                    TextureFormat::SharedBGRA(shared_id) => {
                        let _: () = msg_send![descriptor.as_id(), setStorageMode: MTLStorageMode::Private];
                        let _: () = msg_send![descriptor.as_id(), setUsage: MTLTextureUsage::RenderTarget];
                        let texture: ObjcId = msg_send![metal_cx.device, newSharedTextureWithDescriptor: descriptor];
                        // lets send this to the other side.
                        let shared: ObjcId = msg_send![texture, makeSharedTextureHandle];
                        // lets send it over
                        store_xpc_service_texture(shared_id, shared);
                        
                        texture
                    }
                    _ => panic!(),
                }
            }).unwrap());
            
            self.inner = Some(CxOsTextureInner {
                is_initial: true,
                width,
                height,
                format: desc.format,
                multisample: desc.multisample,
                texture,
            });
            
            if desc.format.is_shared() {
                return
            }
        }
        
        let inner = self.inner.as_ref().unwrap();
        
        // ok now update the texture
        let region = MTLRegion {
            origin: MTLOrigin {x: 0, y: 0, z: 0},
            size: MTLSize {width: width as u64, height: height as u64, depth: 1}
        };
        
        let () = unsafe {msg_send![
            inner.texture.as_id(),
            replaceRegion: region
            mipmapLevel: 0
            withBytes: data.as_ptr() as *const std::ffi::c_void
            bytesPerRow: (width * std::mem::size_of::<u32>() as u64)
        ]};
    }
    
    
    fn update_shared_texture(
        &mut self,
        metal_cx: &MetalCx,
        desc: &TextureDesc,
    ) {
        // we need a width/height for this one.
        if desc.width.is_none() || desc.height.is_none() {
            log!("Shared texture width/height is undefined, cannot allocate it");
            return
        }
        
        let width = desc.width.unwrap() as u64;
        let height = desc.height.unwrap() as u64;
        
        let need_alloc = if let Some(inner) = &self.inner {
            CxOsTextureInner::need_alloc(width, height, desc, inner)
        }
        else {
            true
        };
        
        if need_alloc {
            let descriptor = RcObjcId::from_owned(NonNull::new(unsafe {
                msg_send![class!(MTLTextureDescriptor), new]
            }).unwrap());
            
            let texture = RcObjcId::from_owned(NonNull::new(unsafe {
                let _: () = msg_send![descriptor.as_id(), setTextureType: MTLTextureType::D2];
                let _: () = msg_send![descriptor.as_id(), setWidth: width as u64];
                let _: () = msg_send![descriptor.as_id(), setHeight: height as u64];
                let _: () = msg_send![descriptor.as_id(), setDepth: 1u64];
                let _: () = msg_send![descriptor.as_id(), setStorageMode: MTLStorageMode::Private];
                let _: () = msg_send![descriptor.as_id(), setUsage: MTLTextureUsage::RenderTarget];
                match desc.format {
                    TextureFormat::SharedBGRA(_shared_id) => {
                        let texture: ObjcId = msg_send![metal_cx.device, newSharedTextureWithDescriptor: descriptor];
                        // lets send this to the other side.
                        let _shared: ObjcId = msg_send![texture, newSharedTextureHandle];
                        // lets send it over
                        //log!("STORING SHARED TEXTURE {}", shared_id);
                        //store_xpc_service_texture(shared_id, shared);
                        texture
                    }
                    _ => panic!(),
                }
            }).unwrap());
            
            self.inner = Some(CxOsTextureInner {
                is_initial: true,
                width,
                height,
                format: desc.format,
                multisample: desc.multisample,
                texture,
            });
        }
    }
    
    pub fn update_from_shared_handle(
        &mut self,
        metal_cx: &MetalCx,
        shared_handle: ObjcId,
        width: u64,
        height: u64,
    ) {
        
        let texture = RcObjcId::from_owned(NonNull::new(unsafe {
            msg_send![metal_cx.device, newSharedTextureWithHandle: shared_handle]
        }).unwrap());
        
        self.inner = Some(CxOsTextureInner {
            is_initial: true,
            width,
            height,
            format: TextureFormat::RenderBGRA,
            multisample: None,
            texture,
        });
    }
    
    fn update_render_target(
        &mut self,
        metal_cx: &MetalCx,
        attachment_kind: AttachmentKind,
        desc: &TextureDesc,
        default_size: DVec2
    ) {
        let width = desc.width.unwrap_or(default_size.x as usize) as u64;
        let height = desc.height.unwrap_or(default_size.y as usize) as u64;
        
        if let Some(inner) = &self.inner {
            if inner.format.is_shared() {
                return;
            }
            if !CxOsTextureInner::need_alloc(width, height, desc, inner) {
                return
            }
        }
        
        let descriptor = RcObjcId::from_owned(NonNull::new(unsafe {
            msg_send![class!(MTLTextureDescriptor), new]
        }).unwrap());
        
        let texture = RcObjcId::from_owned(NonNull::new(unsafe {
            let _: () = msg_send![descriptor.as_id(), setTextureType: MTLTextureType::D2];
            let _: () = msg_send![descriptor.as_id(), setWidth: width as u64];
            let _: () = msg_send![descriptor.as_id(), setHeight: height as u64];
            let _: () = msg_send![descriptor.as_id(), setDepth: 1u64];
            let _: () = msg_send![descriptor.as_id(), setStorageMode: MTLStorageMode::Private];
            let _: () = msg_send![descriptor.as_id(), setUsage: MTLTextureUsage::RenderTarget];
            match attachment_kind {
                AttachmentKind::Color => {
                    match desc.format {
                        TextureFormat::RenderBGRA | TextureFormat::Default => {
                            let _: () = msg_send![
                                descriptor.as_id(),
                                setPixelFormat: MTLPixelFormat::BGRA8Unorm
                            ];
                        }
                        _ => panic!(),
                    }
                }
                AttachmentKind::Depth => {
                    match desc.format {
                        TextureFormat::Depth32Stencil8 | TextureFormat::Default => {
                            let _: () = msg_send![
                                descriptor.as_id(),
                                setPixelFormat: MTLPixelFormat::Depth32Float_Stencil8
                            ];
                        }
                        _ => panic!("{:?}", desc.format),
                    }
                }
            }
            msg_send![metal_cx.device, newTextureWithDescriptor: descriptor]
        }).unwrap());
        
        self.inner = Some(CxOsTextureInner {
            is_initial: true,
            width,
            height,
            format: desc.format,
            multisample: desc.multisample,
            texture,
        });
    }
}

struct CxOsTextureInner {
    is_initial: bool,
    width: u64,
    height: u64,
    format: TextureFormat,
    multisample: Option<usize>,
    
    
    texture: RcObjcId
}

impl CxOsTextureInner {
    fn need_alloc(width: u64, height: u64, desc: &TextureDesc, inner: &CxOsTextureInner) -> bool {
        if inner.width != width {
            return true;
        }
        if inner.height != height {
            return true;
        }
        if inner.format != desc.format {
            return true;
        }
        if inner.multisample != desc.multisample {
            return true;
        }
        false
    }
    
    fn initial(&mut self) -> bool {
        let ret = self.is_initial;
        self.is_initial = false;
        ret
    }
}

enum AttachmentKind {
    Color,
    Depth,
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
    #[cfg(target_os = "ios")]
    {
        MTLCreateSystemDefaultDevice().into_iter().collect()
    }
    #[cfg(not(target_os = "ios"))]
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


