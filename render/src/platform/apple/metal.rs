use {
    makepad_objc_sys::{
        msg_send,
        runtime::{YES, NO},
        sel,
        class,
        sel_impl,
    },
    makepad_shader_compiler::{
        generate_metal,
        generate_metal::MetalGeneratedShader,
    },
    makepad_live_compiler::*,
    crate::{
        platform::{
            apple::frameworks::*,
            cocoa_app::CocoaApp,
            cocoa_window::CocoaWindow,
        },
        event::WindowGeom,
        cx::Cx,
        pass::{PassClearColor, PassClearDepth},
        texture::{
            TextureFormat,
            TextureDesc,
            CxTexture
        },
    },
};

impl Cx {
    
    pub fn render_view(
        &mut self,
        pass_id: usize,
        view_id: usize,
        scroll: Vec2,
        clip: (Vec2, Vec2),
        zbias: &mut f32,
        zbias_step: f32,
        encoder: ObjcId,
        metal_cx: &MetalCx,
    ) {
        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
        let draw_items_len = self.views[view_id].draw_items_len;
        //self.views[view_id].set_clipping_uniforms();
        self.views[view_id].uniform_view_transform(&Mat4::identity());
        self.views[view_id].parent_scroll = scroll;
        let local_scroll = self.views[view_id].get_local_scroll();
        let clip = self.views[view_id].intersect_clip(clip);
        
        for draw_item_id in 0..draw_items_len {
            if let Some(sub_view_id) = self.views[view_id].draw_items[draw_item_id].sub_view_id {
                self.render_view(
                    pass_id,
                    sub_view_id,
                    Vec2 {x: local_scroll.x + scroll.x, y: local_scroll.y + scroll.y},
                    clip,
                    zbias,
                    zbias_step,
                    encoder,
                    metal_cx,
                );
            }
            else {
                let cxview = &mut self.views[view_id];
                //view.platform.uni_vw.update_with_f32_data(device, &view.uniforms);
                let draw_call = cxview.draw_items[draw_item_id].draw_call.as_mut().unwrap();
                let sh = &self.draw_shaders[draw_call.draw_shader.draw_shader_id];
                if sh.platform.inner.is_none() { // shader didnt compile somehow
                    continue;
                }
                let shp = sh.platform.inner.as_ref().unwrap();
                
                if draw_call.instance_dirty {
                    draw_call.instance_dirty = false;
                    // update the instance buffer data
                    self.platform.bytes_written += draw_call.instances.as_ref().unwrap().len() * 4;
                    draw_call.platform.inst_vbuf.update(metal_cx, &draw_call.instances.as_ref().unwrap());
                }
                
                // update the zbias uniform if we have it.
                draw_call.set_zbias(*zbias);
                draw_call.set_local_scroll(scroll, local_scroll);
                draw_call.set_clip(clip);
                *zbias += zbias_step;
                
                if draw_call.uniforms_dirty {
                    draw_call.uniforms_dirty = false;
                }
                
                // lets verify our instance_offset is not disaligned
                let instances = (draw_call.instances.as_ref().unwrap().len() / sh.mapping.instances.total_slots) as u64;
                if instances == 0 {
                    continue;
                }
                let render_pipeline_state = shp.render_pipeline_state.as_id();
                unsafe {let () = msg_send![encoder, setRenderPipelineState: render_pipeline_state];}
                
                let geometry_id = if let Some(geometry_id) = draw_call.geometry_id {geometry_id}
                else {
                    continue;
                };
                // else if let Some(geometry) = sh.default_geometry{
                //    geometry.geometry_id
                //}
                // else{
                //     continue
                //};
                
                let geometry = &mut self.geometries[geometry_id];
                
                if geometry.dirty {
                    geometry.platform.geom_ibuf.update(metal_cx, &geometry.indices);
                    geometry.platform.geom_vbuf.update(metal_cx, &geometry.vertices);
                    geometry.dirty = false;
                }
                
                if let Some(inner) = geometry.platform.geom_vbuf.inner.as_ref() {
                    unsafe {msg_send![
                        encoder,
                        setVertexBuffer: inner.buffer.as_id()
                        offset: 0
                        atIndex: 0
                    ]}
                }
                else {println!("Drawing error: geom_vbuf None")}
                
                if let Some(inner) = draw_call.platform.inst_vbuf.inner.as_ref() {
                    unsafe {msg_send![
                        encoder,
                        setVertexBuffer: inner.buffer.as_id()
                        offset: 0
                        atIndex: 1
                    ]}
                }
                else {println!("Drawing error: inst_vbuf None")}
                
                let pass_uniforms = self.passes[pass_id].pass_uniforms.as_slice();
                let view_uniforms = cxview.view_uniforms.as_slice();
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
                        let () = msg_send![encoder, setVertexBytes: view_uniforms.as_ptr() as *const std::ffi::c_void length: (view_uniforms.len() * 4) as u64 atIndex: id];
                        let () = msg_send![encoder, setFragmentBytes: view_uniforms.as_ptr() as *const std::ffi::c_void length: (view_uniforms.len() * 4) as u64 atIndex: id];
                    }
                    if let Some(id) = shp.user_uniform_buffer_id {
                        let () = msg_send![encoder, setVertexBytes: draw_call.user_uniforms.as_ptr() as *const std::ffi::c_void length: (draw_call.user_uniforms.len() * 4) as u64 atIndex: id];
                        let () = msg_send![encoder, setFragmentBytes: draw_call.user_uniforms.as_ptr() as *const std::ffi::c_void length: (draw_call.user_uniforms.len() * 4) as u64 atIndex: id];
                    }
                    
                    let ct = &sh.mapping.const_table.table;
                    if ct.len()>0 {
                        let () = msg_send![encoder, setVertexBytes: ct.as_ptr() as *const std::ffi::c_void length: (ct.len() * 4) as u64 atIndex: 7u64];
                        let () = msg_send![encoder, setFragmentBytes: ct.as_ptr() as *const std::ffi::c_void length: (ct.len() * 4) as u64 atIndex: 5u64];
                    }
                }
                //encoder.set_vertex_bytes(2, (pass_uniforms.len() * 4) as u64, pass_uniforms.as_ptr() as *const std::ffi::c_void);
                //encoder.set_vertex_bytes(3, (view_uniforms.len() * 4) as u64, view_uniforms.as_ptr() as *const std::ffi::c_void);
                //encoder.set_vertex_bytes(4, (draw_uniforms.len() * 4) as u64, draw_uniforms.as_ptr() as *const std::ffi::c_void);
                //encoder.set_vertex_bytes(5, (draw_call.uniforms.len() * 4) as u64, draw_call.uniforms.as_ptr() as *const std::ffi::c_void);
                //encoder.set_fragment_bytes(0, (pass_uniforms.len() * 4) as u64, pass_uniforms.as_ptr() as *const std::ffi::c_void);
                //encoder.set_fragment_bytes(1, (view_uniforms.len() * 4) as u64, view_uniforms.as_ptr() as *const std::ffi::c_void);
                //encoder.set_fragment_bytes(2, (draw_uniforms.len() * 4) as u64, draw_uniforms.as_ptr() as *const std::ffi::c_void);
                //encoder.set_fragment_bytes(3, (draw_call.uniforms.len() * 4) as u64, draw_call.uniforms.as_ptr() as *const std::ffi::c_void);
                // lets set our textures
                for i in 0..sh.mapping.textures.len() {
                    
                    let texture_id = if let Some(texture_id) = draw_call.texture_slots[i] {
                        texture_id
                    }else {0};
                    
                    let cxtexture = &mut self.textures[texture_id as usize];
                    if cxtexture.update_image {
                        metal_cx.update_platform_texture_image2d(cxtexture);
                    }
                    if let Some(mtl_texture) = cxtexture.platform.mtl_texture {
                        let () = unsafe {msg_send![
                            encoder,
                            setFragmentTexture: mtl_texture
                            atIndex: i as u64
                        ]};
                        let () = unsafe {msg_send![
                            encoder,
                            setVertexTexture: mtl_texture
                            atIndex: i as u64
                        ]};
                    }
                }
                self.platform.draw_calls_done += 1;
                if let Some(inner) = geometry.platform.geom_ibuf.inner.as_ref() {
                    
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
                else {println!("Drawing error: geom_ibuf None")}
            }
        }
    }
    
    pub fn setup_render_pass_descriptor(&mut self, render_pass_descriptor: ObjcId, pass_id: usize, inherit_dpi_factor: f32, first_texture: Option<ObjcId>, metal_cx: &MetalCx) {
        let pass_size = self.passes[pass_id].pass_size;
        
        self.passes[pass_id].set_matrix(Vec2::default(), pass_size);
        self.passes[pass_id].paint_dirty = false;
        let dpi_factor = if let Some(override_dpi_factor) = self.passes[pass_id].override_dpi_factor {
            override_dpi_factor
        }
        else {
            inherit_dpi_factor
        };
        self.passes[pass_id].set_dpi_factor(dpi_factor);
        
        for (index, color_texture) in self.passes[pass_id].color_textures.iter().enumerate() {
            let color_attachments: ObjcId = unsafe {msg_send![render_pass_descriptor, colorAttachments]};
            let color_attachment: ObjcId = unsafe {msg_send![color_attachments, objectAtIndexedSubscript: 0]};
            // let color_attachment = render_pass_descriptor.color_attachments().object_at(0).unwrap();
            
            let is_initial;
            if index == 0 && first_texture.is_some() {
                let () = unsafe {msg_send![
                    color_attachment,
                    setTexture: first_texture.unwrap()
                ]};
                is_initial = true;
            }
            else {
                let cxtexture = &mut self.textures[color_texture.texture_id as usize];
                is_initial = metal_cx.update_platform_render_target(cxtexture, dpi_factor, pass_size, false);
                
                if let Some(mtl_texture) = cxtexture.platform.mtl_texture {
                    let () = unsafe {msg_send![
                        color_attachment,
                        setTexture: mtl_texture
                    ]};
                }
                else {
                    println!("draw_pass_to_texture invalid render target");
                }
                
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
        // attach depth texture
        if let Some(depth_texture_id) = self.passes[pass_id].depth_texture {
            let cxtexture = &mut self.textures[depth_texture_id as usize];
            let is_initial = metal_cx.update_platform_render_target(cxtexture, dpi_factor, pass_size, true);
            
            let depth_attachment: ObjcId = unsafe {msg_send![render_pass_descriptor, depthAttachment]};
            
            if let Some(mtl_texture) = cxtexture.platform.mtl_texture {
                unsafe {msg_send![depth_attachment, setTexture: mtl_texture]}
            }
            else {
                println!("draw_pass_to_texture invalid render target");
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
    }
    
    pub fn draw_pass_to_layer(
        &mut self,
        pass_id: usize,
        dpi_factor: f32,
        layer: ObjcId,
        metal_cx: &mut MetalCx,
    ) {
        self.platform.bytes_written = 0;
        self.platform.draw_calls_done = 0;
        let view_id = self.passes[pass_id].main_view_id.unwrap();
        
        let pool: ObjcId = unsafe {msg_send![class!(NSAutoreleasePool), new]};
        
        //let command_buffer = command_queue.new_command_buffer();
        let drawable: ObjcId = unsafe {msg_send![layer, nextDrawable]};
        if drawable != nil {
            let render_pass_descriptor: ObjcId = unsafe {msg_send![class!(MTLRenderPassDescriptorInternal), renderPassDescriptor]};
            
            let texture: ObjcId = unsafe {msg_send![drawable, texture]};
            
            self.setup_render_pass_descriptor(render_pass_descriptor, pass_id, dpi_factor, Some(texture), metal_cx);
            
            let command_buffer: ObjcId = unsafe {msg_send![metal_cx.command_queue, commandBuffer]};
            let encoder: ObjcId = unsafe {msg_send![command_buffer, renderCommandEncoderWithDescriptor: render_pass_descriptor]};
            
            unsafe {msg_send![encoder, textureBarrier]}
            
            if let Some(depth_state) = self.passes[pass_id].platform.mtl_depth_state {
                let () = unsafe {msg_send![encoder, setDepthStencilState: depth_state]};
            }
            let mut zbias = 0.0;
            let zbias_step = self.passes[pass_id].zbias_step;
            
            self.render_view(
                pass_id,
                view_id,
                Vec2::default(),
                (Vec2 {x: -50000., y: -50000.}, Vec2 {x: 50000., y: 50000.}),
                &mut zbias,
                zbias_step,
                encoder,
                &metal_cx,
            );
            
            let () = unsafe {msg_send![encoder, endEncoding]};
            let () = unsafe {msg_send![command_buffer, presentDrawable: drawable]};
            let () = unsafe {msg_send![command_buffer, commit]};
            let () = unsafe {msg_send![command_buffer, waitUntilScheduled]};
        }
        let () = unsafe {msg_send![pool, release]};
    } 
    
    pub fn draw_pass_to_texture(
        &mut self,
        pass_id: usize,
        dpi_factor: f32,
        metal_cx: &MetalCx,
    ) {
        let view_id = self.passes[pass_id].main_view_id.unwrap();
        
        let pool: ObjcId = unsafe {msg_send![class!(NSAutoreleasePool), new]};
        let render_pass_descriptor: ObjcId = unsafe {msg_send![class!(MTLRenderPassDescriptorInternal), renderPassDescriptor]};
        
        self.setup_render_pass_descriptor(render_pass_descriptor, pass_id, dpi_factor, None, metal_cx);
        
        let command_buffer: ObjcId = unsafe {msg_send![metal_cx.command_queue, commandBuffer]};
        let encoder: ObjcId = unsafe {msg_send![command_buffer, renderCommandEncoderWithDescriptor: render_pass_descriptor]};
        
        if let Some(depth_state) = self.passes[pass_id].platform.mtl_depth_state {
            let () = unsafe {msg_send![encoder, setDepthStencilState: depth_state]};
        }
        
        let mut zbias = 0.0;
        let zbias_step = self.passes[pass_id].zbias_step;
        self.render_view(
            pass_id,
            view_id,
            Vec2::default(),
            (Vec2 {x: -50000., y: -50000.}, Vec2 {x: 50000., y: 50000.}),
            &mut zbias,
            zbias_step,
            encoder,
            &metal_cx,
        );
        let () = unsafe {msg_send![encoder, textureBarrier]};
        let () = unsafe {msg_send![encoder, endEncoding]};
        let () = unsafe {msg_send![command_buffer, commit]};
        let () = unsafe {msg_send![command_buffer, waitUntilScheduled]};
        let () = unsafe {msg_send![pool, release]};
    }
}

pub struct MetalCx {
    pub device: ObjcId,
    pub command_queue: ObjcId
}


#[derive(Clone)]
pub struct MetalWindow {
    pub window_id: usize,
    pub first_draw: bool,
    pub window_geom: WindowGeom,
    pub cal_size: Vec2,
    pub ca_layer: ObjcId,
    pub cocoa_window: CocoaWindow,
}

impl MetalWindow {
    pub fn new(window_id: usize, metal_cx: &MetalCx, cocoa_app: &mut CocoaApp, inner_size: Vec2, position: Option<Vec2>, title: &str) -> MetalWindow {
        
        let ca_layer: ObjcId = unsafe {msg_send![class!(CAMetalLayer), new]};
        
        let mut cocoa_window = CocoaWindow::new(cocoa_app, window_id);
        
        cocoa_window.init(title, inner_size, position);
        
        unsafe {
            let () = msg_send![ca_layer, setDevice: metal_cx.device];
            let () = msg_send![ca_layer, setPixelFormat: MTLPixelFormat::BGRA8Unorm];
            let () = msg_send![ca_layer, setPresentsWithTransaction: NO];
            let () = msg_send![ca_layer, setMaximumDrawableCount: 3];
            let () = msg_send![ca_layer, setDisplaySyncEnabled: NO];
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
            first_draw: true,
            window_id,
            cal_size: Vec2::default(), 
            ca_layer,
            window_geom: cocoa_window.get_window_geom(),
            cocoa_window
        }
    }
    
    pub fn set_vsync_enable(&mut self, _enable: bool) {
       // let () = unsafe {msg_send![self.ca_layer, setDisplaySyncEnabled: false]};
    }
    
    pub fn set_buffer_count(&mut self, _count: u64) {
      //  let () = unsafe {msg_send![self.ca_layer, setMaximumDrawableCount: 3]};
    }
    
    pub fn resize_core_animation_layer(&mut self, _metal_cx: &MetalCx) -> bool {
        let cal_size = Vec2 {
            x: self.window_geom.inner_size.x * self.window_geom.dpi_factor,
            y: self.window_geom.inner_size.y * self.window_geom.dpi_factor
        };
        if self.cal_size != cal_size {
            self.cal_size = cal_size;
            unsafe {
                let () = msg_send![self.ca_layer, setDrawableSize: CGSize {width: cal_size.x as f64, height: cal_size.y as f64}];
                let () = msg_send![self.ca_layer, setContentsScale: self.window_geom.dpi_factor as f64];
            }
            true
        }
        else {
            false
        }
    }
    
}

#[derive(Clone, Default)]
pub struct CxPlatformView {
}

#[derive(Default)]
pub struct CxPlatformDrawCall {
    //pub uni_dr: MetalBuffer,
    inst_vbuf: MetalBuffer
}

#[derive(Default, Clone)]
pub struct CxPlatformTexture {
    pub alloc_desc: TextureDesc,
    pub width: u64,
    pub height: u64,
    pub mtl_texture: Option<ObjcId>
}

#[derive(Default, Clone)]
pub struct CxPlatformPass {
    pub mtl_depth_state: Option<ObjcId>
}

#[derive(Default)]
pub struct CxPlatformGeometry {
    geom_vbuf: MetalBuffer,
    geom_ibuf: MetalBuffer,
}

pub enum PackType {
    Packed,
    Unpacked
}

pub struct SlErr {
    msg: String
}

impl Cx {
    
    pub fn mtl_compile_shaders(&mut self, metal_cx: &MetalCx) {
        for draw_shader_ptr in &self.draw_shader_compile_set {
            if let Some(draw_shader_id) = self.draw_shader_ptr_to_id.get(&draw_shader_ptr) {
                let cx_shader = &mut self.draw_shaders[*draw_shader_id];
                let draw_shader_def = self.shader_registry.draw_shader_defs.get(&draw_shader_ptr);
                let gen = generate_metal::generate_shader(
                    draw_shader_def.as_ref().unwrap(),
                    &cx_shader.mapping.const_table,
                    &self.shader_registry
                );
                cx_shader.platform.update(metal_cx, gen);
            }
        }
        self.draw_shader_compile_set.clear();
    }
}

impl MetalCx {
    
    pub fn new() -> MetalCx {
        /*
        let devices = get_all_metal_devices();
        for device in devices {
            let is_low_power: BOOL = unsafe {msg_send![device, isLowPower]};
            let command_queue: id = unsafe {msg_send![device, newCommandQueue]};
            if is_low_power == YES {
                return MetalCx {
                    command_queue: command_queue,
                    device: device
                }
            }
        }*/
        let device = get_default_metal_device().expect("Cannot get default metal device");
        MetalCx {
            command_queue: unsafe {msg_send![device, newCommandQueue]},
            device: device
        }
    }
    
    pub fn update_platform_render_target(&self, cxtexture: &mut CxTexture, dpi_factor: f32, size: Vec2, is_depth: bool) -> bool {
        
        let width = if let Some(width) = cxtexture.desc.width {width as u64} else {(size.x * dpi_factor) as u64};
        let height = if let Some(height) = cxtexture.desc.height {height as u64} else {(size.y * dpi_factor) as u64};
        
        if cxtexture.platform.width == width && cxtexture.platform.height == height && cxtexture.platform.alloc_desc == cxtexture.desc {
            return false
        }
        cxtexture.platform.mtl_texture = None;
        
        let mdesc: ObjcId = unsafe {msg_send![class!(MTLTextureDescriptor), new]};
        if !is_depth {
            match cxtexture.desc.format {
                TextureFormat::Default | TextureFormat::RenderBGRA => {
                    unsafe {
                        let () = msg_send![mdesc, setPixelFormat: MTLPixelFormat::BGRA8Unorm];
                        let () = msg_send![mdesc, setTextureType: MTLTextureType::D2];
                        let () = msg_send![mdesc, setStorageMode: MTLStorageMode::Private];
                        let () = msg_send![mdesc, setUsage: MTLTextureUsage::RenderTarget];
                    }
                },
                _ => {
                    println!("update_platform_render_target unsupported texture format");
                    return false;
                }
            }
        }
        else {
            match cxtexture.desc.format {
                TextureFormat::Default | TextureFormat::Depth32Stencil8 => {
                    unsafe {
                        let () = msg_send![mdesc, setPixelFormat: MTLPixelFormat::Depth32Float_Stencil8];
                        let () = msg_send![mdesc, setTextureType: MTLTextureType::D2];
                        let () = msg_send![mdesc, setStorageMode: MTLStorageMode::Private];
                        let () = msg_send![mdesc, setUsage: MTLTextureUsage::RenderTarget];
                    }
                },
                _ => {
                    println!("update_platform_render_targete unsupported texture format");
                    return false;
                }
            }
        }
        let () = unsafe {msg_send![mdesc, setWidth: width as u64]};
        let () = unsafe {msg_send![mdesc, setHeight: height as u64]};
        let () = unsafe {msg_send![mdesc, setDepth: 1u64]};
        
        let tex: ObjcId = unsafe {msg_send![self.device, newTextureWithDescriptor: mdesc]};
        
        cxtexture.platform.width = width;
        cxtexture.platform.height = height;
        cxtexture.platform.alloc_desc = cxtexture.desc.clone();
        cxtexture.platform.mtl_texture = Some(tex);
        return true
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
            cxtexture.platform.mtl_texture = None;
            
            let mdesc: ObjcId = unsafe {msg_send![class!(MTLTextureDescriptor), new]};
            unsafe {
                let () = msg_send![mdesc, setTextureType: MTLTextureType::D2];
                let () = msg_send![mdesc, setStorageMode: MTLStorageMode::Managed];
                let () = msg_send![mdesc, setUsage: MTLTextureUsage::RenderTarget];
                let () = msg_send![mdesc, setWidth: width as u64];
                let () = msg_send![mdesc, setHeight: height as u64];
            }
            
            match cxtexture.desc.format {
                TextureFormat::Default | TextureFormat::ImageBGRA => {
                    let () = unsafe {msg_send![mdesc, setPixelFormat: MTLPixelFormat::BGRA8Unorm]};
                    
                    let tex: ObjcId = unsafe {msg_send![self.device, newTextureWithDescriptor: mdesc]};
                    
                    cxtexture.platform.mtl_texture = Some(tex);
                    
                    if cxtexture.image_u32.len() != width * height {
                        println!("update_platform_texture_image2d with wrong buffer_u32 size!");
                        cxtexture.platform.mtl_texture = None;
                        return;
                    }
                    let region = MTLRegion {
                        origin: MTLOrigin {x: 0, y: 0, z: 0},
                        size: MTLSize {width: width as u64, height: height as u64, depth: 1}
                    };
                    if let Some(mtl_texture) = cxtexture.platform.mtl_texture {
                        let () = unsafe {msg_send![
                            mtl_texture,
                            replaceRegion: region
                            mipmapLevel: 0
                            withBytes: cxtexture.image_u32.as_ptr() as *const std::ffi::c_void
                            bytesPerRow: (width * std::mem::size_of::<u32>()) as u64
                        ]};
                    }
                },
                _ => {
                    println!("update_platform_texture_image2d with unsupported format");
                    return;
                }
            }
            cxtexture.platform.alloc_desc = cxtexture.desc.clone();
            cxtexture.platform.width = width as u64;
            cxtexture.platform.height = height as u64;
        }
        
        cxtexture.update_image = false;
    }
}

/**************************************************************************************************/

#[derive(Default)]
pub struct CxPlatformShader {
    inner: Option<CxPlatformShaderInner>
}

impl CxPlatformShader {
    pub fn update(
        &mut self,
        metal_cx: &MetalCx,
        shader: MetalGeneratedShader,
    ) { 
        let options = RcObjcId::from_owned(unsafe {msg_send![class!(MTLCompileOptions), new]});
        unsafe {
            let _: () = msg_send![options.as_id(), setFastMathEnabled: YES];
        };
        
        let mut error: ObjcId = nil;

        let library = RcObjcId::from_owned(match NonNull::new(unsafe {
            msg_send![
                metal_cx.device,
                newLibraryWithSource: str_to_ns_string(&shader.mtlsl)
                options: options
                error: &mut error
            ]
        }) {
            Some(library) => library,
            None => {
                let description: ObjcId = unsafe {msg_send![error, localizedDescription]};
                panic!("{}", nsstring_to_string(description));
            }
        });

        let descriptor = RcObjcId::from_owned(NonNull::new(unsafe {
            msg_send![class!(MTLRenderPipelineDescriptor), new]
        }).unwrap());
        let vertex_function = RcObjcId::from_owned(NonNull::new(unsafe {
            msg_send![library.as_id(), newFunctionWithName: str_to_ns_string("vertex_main")]
        }).unwrap());
        let fragment_function = RcObjcId::from_owned(NonNull::new(unsafe {
            msg_send![library.as_id(), newFunctionWithName: str_to_ns_string("fragment_main")]
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
                id!(draw) => draw_uniform_buffer_id = Some(buffer_id),
                id!(pass) => pass_uniform_buffer_id = Some(buffer_id),
                id!(view) => view_uniform_buffer_id = Some(buffer_id),
                id!(user) => user_uniform_buffer_id = Some(buffer_id),
                _ => panic!()
            }
            buffer_id += 1;
        }
        
        self.inner = Some(CxPlatformShaderInner {
            library,
            render_pipeline_state,
            draw_uniform_buffer_id,
            pass_uniform_buffer_id,
            view_uniform_buffer_id,
            user_uniform_buffer_id,
            source: shader.mtlsl,
        });
    }
}

struct CxPlatformShaderInner {
    library: RcObjcId,
    render_pipeline_state: RcObjcId,
    draw_uniform_buffer_id: Option<u64>,
    pass_uniform_buffer_id: Option<u64>,
    view_uniform_buffer_id: Option<u64>,
    user_uniform_buffer_id: Option<u64>,
    source: String,
}

#[derive(Default)]
struct MetalBuffer {
    inner: Option<MetalBufferInner>,
}

impl MetalBuffer {
    pub fn update<T>(&mut self, metal_cx: &MetalCx, data: &[T]) {
        let len = data.len() * std::mem::size_of::<T>();
        if len == 0 {
            self.inner = None;
            return;
        }
        if self.inner.as_ref().map_or(0, |inner| inner.len) < len {
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
            std::ptr::copy(data.as_ptr() as *const u8, contents, len);
            let () = msg_send![
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