//use cocoa::base::{id};
//use cocoa::appkit::{NSView};
//use cocoa::foundation::{NSAutoreleasePool, NSUInteger, NSRange};
//use core_graphics::geometry::CGSize;
//use core_graphics::color::CGColor;
use makepad_objc_sys::{msg_send};
use makepad_objc_sys::runtime::YES;
//use metal::*;
use crate::cx_apple::*;
use crate::cx_cocoa::*;
use crate::cx::*;

impl Cx {
    
    pub fn render_view(
        &mut self,
        pass_id: usize,
        view_id: usize,
        scroll: Vec2,
        clip: (Vec2, Vec2),
        zbias: &mut f32,
        zbias_step: f32,
        encoder: id,
        metal_cx: &MetalCx,
    ) {
        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
        let draw_calls_len = self.views[view_id].draw_calls_len;
        //self.views[view_id].set_clipping_uniforms();
        self.views[view_id].uniform_view_transform(&Mat4::identity());
        self.views[view_id].parent_scroll = scroll;
        let local_scroll = self.views[view_id].get_local_scroll();
        let clip = self.views[view_id].intersect_clip(clip);
        
        for draw_call_id in 0..draw_calls_len {
            let sub_view_id = self.views[view_id].draw_calls[draw_call_id].sub_view_id;
            if sub_view_id != 0 {
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
                let draw_call = &mut cxview.draw_calls[draw_call_id];
                let sh = &self.shaders[draw_call.shader_id];
                let shp = sh.platform.as_ref().unwrap();
                
                if draw_call.instance_dirty {
                    draw_call.instance_dirty = false;
                    // update the instance buffer data
                    self.platform.bytes_written += draw_call.instance.len() * 4;
                    draw_call.platform.inst_vbuf.update_with_f32_data(metal_cx, &draw_call.instance);
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
                let instances = (draw_call.instance.len() / sh.mapping.instance_slots) as u64;
                if instances == 0 {
                    continue;
                }
                let pipeline_state = shp.pipeline_state;
                unsafe {let () = msg_send![encoder, setRenderPipelineState: pipeline_state];}
                
                if let Some(buf) = shp.geom_vbuf.multi_buffer_read().buffer {
                    unsafe {msg_send![
                        encoder,
                        setVertexBuffer: buf
                        offset: 0
                        atIndex: 0
                    ]}
                }
                else {println!("Drawing error: geom_vbuf None")}
                
                if let Some(buf) = draw_call.platform.inst_vbuf.multi_buffer_read().buffer {
                    unsafe {msg_send![
                        encoder,
                        setVertexBuffer: buf
                        offset: 0
                        atIndex: 1
                    ]}
                }
                else {println!("Drawing error: inst_vbuf None")}
                
                let pass_uniforms = self.passes[pass_id].pass_uniforms.as_slice();
                let view_uniforms = cxview.view_uniforms.as_slice();
                let draw_uniforms = draw_call.draw_uniforms.as_slice();
                
                unsafe {
                    let () = msg_send![encoder, setVertexBytes: pass_uniforms.as_ptr() as *const std::ffi::c_void length: (pass_uniforms.len() * 4) as u64 atIndex: 2u64];
                    let () = msg_send![encoder, setVertexBytes: view_uniforms.as_ptr() as *const std::ffi::c_void length: (view_uniforms.len() * 4) as u64 atIndex: 3u64];
                    let () = msg_send![encoder, setVertexBytes: draw_uniforms.as_ptr() as *const std::ffi::c_void length: (draw_uniforms.len() * 4) as u64 atIndex: 4u64];
                    let () = msg_send![encoder, setVertexBytes: draw_call.uniforms.as_ptr() as *const std::ffi::c_void length: (draw_call.uniforms.len() * 4) as u64 atIndex: 5u64];
                    let () = msg_send![encoder, setFragmentBytes: pass_uniforms.as_ptr() as *const std::ffi::c_void length: (pass_uniforms.len() * 4) as u64 atIndex: 0u64];
                    let () = msg_send![encoder, setFragmentBytes: view_uniforms.as_ptr() as *const std::ffi::c_void length: (view_uniforms.len() * 4) as u64 atIndex: 1u64];
                    let () = msg_send![encoder, setFragmentBytes: draw_uniforms.as_ptr() as *const std::ffi::c_void length: (draw_uniforms.len() * 4) as u64 atIndex: 2u64];
                    let () = msg_send![encoder, setFragmentBytes: draw_call.uniforms.as_ptr() as *const std::ffi::c_void length: (draw_call.uniforms.len() * 4) as u64 atIndex: 3u64];
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
                for (i, texture_id) in draw_call.textures_2d.iter().enumerate() {
                    let cxtexture = &mut self.textures[*texture_id as usize];
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
                if let Some(buf) = shp.geom_ibuf.multi_buffer_read().buffer {
                    
                    let () = unsafe {msg_send![
                        encoder,
                        drawIndexedPrimitives: MTLPrimitiveType::Triangle
                        indexCount: sh.shader_gen.geometry_indices.len() as u64
                        indexType: MTLIndexType::UInt32
                        indexBuffer: buf
                        indexBufferOffset: 0
                        instanceCount: instances
                    ]};
                }
                else {println!("Drawing error: geom_ibuf None")}
            }
        }
    }
    
    pub fn setup_render_pass_descriptor(&mut self, render_pass_descriptor: id, pass_id: usize, inherit_dpi_factor: f32, first_texture: Option<id>, metal_cx: &MetalCx) {
        let pass_size = self.passes[pass_id].pass_size;
        
        self.passes[pass_id].set_ortho_matrix(Vec2::default(), pass_size);
        self.passes[pass_id].uniform_camera_view(&Mat4::identity());
        self.passes[pass_id].paint_dirty = false;
        let dpi_factor = if let Some(override_dpi_factor) = self.passes[pass_id].override_dpi_factor {
            override_dpi_factor
        }
        else {
            inherit_dpi_factor
        };
        self.passes[pass_id].set_dpi_factor(dpi_factor);
        
        for (index, color_texture) in self.passes[pass_id].color_textures.iter().enumerate() {
            let color_attachments: id = unsafe {msg_send![render_pass_descriptor, colorAttachments]};
            let color_attachment: id = unsafe {msg_send![color_attachments, objectAtIndexedSubscript: 0]};
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
                let cxtexture = &mut self.textures[color_texture.texture_id];
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
                ClearColor::InitWith(color) => {
                    if is_initial {
                        unsafe {
                            let () = msg_send![color_attachment, setLoadAction: MTLLoadAction::Clear];
                            let () = msg_send![color_attachment, setClearColor: MTLClearColor {
                                red: color.r as f64,
                                green: color.g as f64,
                                blue: color.b as f64,
                                alpha: color.a as f64
                            }];
                        }
                    }
                    else {
                        unsafe {let () = msg_send![color_attachment, setLoadAction: MTLLoadAction::Load];}
                    }
                },
                ClearColor::ClearWith(color) => {
                    unsafe {
                        let () = msg_send![color_attachment, setLoadAction: MTLLoadAction::Clear];
                        let () = msg_send![color_attachment, setClearColor: MTLClearColor {
                            red: color.r as f64,
                            green: color.g as f64,
                            blue: color.b as f64,
                            alpha: color.a as f64
                        }];
                    }
                }
            }
        }
        // attach depth texture
        if let Some(depth_texture_id) = self.passes[pass_id].depth_texture {
            let cxtexture = &mut self.textures[depth_texture_id];
            let is_initial = metal_cx.update_platform_render_target(cxtexture, dpi_factor, pass_size, true);
            
            let depth_attachment: id = unsafe {msg_send![render_pass_descriptor, depthAttachment]};
            
            if let Some(mtl_texture) = cxtexture.platform.mtl_texture {
                unsafe {msg_send![depth_attachment, setTexture: mtl_texture]}
            }
            else {
                println!("draw_pass_to_texture invalid render target");
            }
            let () = unsafe {msg_send![depth_attachment, setStoreAction: MTLStoreAction::Store]};
            
            match self.passes[pass_id].clear_depth {
                ClearDepth::InitWith(depth) => {
                    if is_initial {
                        let () = unsafe {msg_send![depth_attachment, setLoadAction: MTLLoadAction::Clear]};
                        let () = unsafe {msg_send![depth_attachment, setClearDepth: depth as f64]};
                    }
                    else {
                        let () = unsafe {msg_send![depth_attachment, setLoadAction: MTLLoadAction::Load]};
                    }
                },
                ClearDepth::ClearWith(depth) => {
                    let () = unsafe {msg_send![depth_attachment, setLoadAction: MTLLoadAction::Clear]};
                    let () = unsafe {msg_send![depth_attachment, setClearDepth: depth as f64]};
                }
            }
            // create depth state
            if self.passes[pass_id].platform.mtl_depth_state.is_none() {
                
                let desc: id = unsafe {msg_send![class!(MTLDepthStencilDescriptor), new]};
                let () = unsafe {msg_send![desc, setDepthCompareFunction: MTLCompareFunction::LessEqual]};
                let () = unsafe {msg_send![desc, setDepthWriteEnabled: true]};
                let depth_stencil_state: id = unsafe {msg_send![metal_cx.device, newDepthStencilStateWithDescriptor: desc]};
                self.passes[pass_id].platform.mtl_depth_state = Some(depth_stencil_state);
            }
        }
    }
    
    pub fn draw_pass_to_layer(
        &mut self,
        pass_id: usize,
        dpi_factor: f32,
        layer: id,
        metal_cx: &mut MetalCx,
    ) {
        self.platform.bytes_written = 0;
        self.platform.draw_calls_done = 0;
        let view_id = self.passes[pass_id].main_view_id.unwrap();
        
        let pool: id = unsafe {msg_send![class!(NSAutoreleasePool), new]};
        
        //let command_buffer = command_queue.new_command_buffer();
        let drawable: id = unsafe {msg_send![layer, nextDrawable]};
        if drawable != nil {
            let render_pass_descriptor: id = unsafe {msg_send![class!(MTLRenderPassDescriptorInternal), renderPassDescriptor]};
            
            let texture: id = unsafe {msg_send![drawable, texture]};
            
            self.setup_render_pass_descriptor(render_pass_descriptor, pass_id, dpi_factor, Some(texture), metal_cx);
            
            let command_buffer: id = unsafe {msg_send![metal_cx.command_queue, commandBuffer]};
            let encoder: id = unsafe {msg_send![command_buffer, renderCommandEncoderWithDescriptor: render_pass_descriptor]};
            
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
            //command_buffer.wait_until_scheduled();
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
        
        let pool: id = unsafe {msg_send![class!(NSAutoreleasePool), new]};
        let render_pass_descriptor: id = unsafe {msg_send![class!(MTLRenderPassDescriptorInternal), renderPassDescriptor]};
        
        self.setup_render_pass_descriptor(render_pass_descriptor, pass_id, dpi_factor, None, metal_cx);
        
        let command_buffer: id = unsafe {msg_send![metal_cx.command_queue, commandBuffer]};
        let encoder: id = unsafe {msg_send![command_buffer, renderCommandEncoderWithDescriptor: render_pass_descriptor]};
        
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
        //command_buffer.wait_until_scheduled();
        let () = unsafe {msg_send![pool, release]};
    }
}

pub struct MetalCx {
    pub device: id,
    pub command_queue: id
}

impl MetalCx {
    
    pub fn new() -> MetalCx {
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
        }
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
        
        let mdesc: id = unsafe {msg_send![class!(MTLTextureDescriptor), new]};
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
        
        let tex: id = unsafe {msg_send![self.device, newTextureWithDescriptor: mdesc]};
        
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
            
            let mdesc: id = unsafe {msg_send![class!(MTLTextureDescriptor), new]};
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
                    
                    let tex: id = unsafe {msg_send![self.device, newTextureWithDescriptor: mdesc]};
                    
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

#[derive(Clone)]
pub struct MetalWindow {
    pub window_id: usize,
    pub first_draw: bool,
    pub window_geom: WindowGeom,
    pub cal_size: Vec2,
    pub ca_layer: id,
    pub cocoa_window: CocoaWindow,
}

impl MetalWindow {
    pub fn new(window_id: usize, metal_cx: &MetalCx, cocoa_app: &mut CocoaApp, inner_size: Vec2, position: Option<Vec2>, title: &str) -> MetalWindow {
        
        let ca_layer: id = unsafe {msg_send![class!(CAMetalLayer), new]};
        
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
    
    pub fn set_vsync_enable(&mut self, enable: bool) {
        let () = unsafe {msg_send![self.ca_layer, setDisplaySyncEnabled: enable]};
    }
    
    pub fn set_buffer_count(&mut self, _count: u64) {
        let () = unsafe {msg_send![self.ca_layer, setMaximumDrawableCount: 3]};
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
            //self.msam_target = Some(RenderTarget::new(device, self.cal_size.x as u64, self.cal_size.y as u64, 2));
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

#[derive(Default, Clone)]
pub struct CxPlatformDrawCall {
    //pub uni_dr: MetalBuffer,
    pub inst_vbuf: MetalBuffer
}

#[derive(Default, Clone)]
pub struct CxPlatformTexture {
    pub alloc_desc: TextureDesc,
    pub width: u64,
    pub height: u64,
    pub mtl_texture: Option<id>
}

#[derive(Default, Clone)]
pub struct CxPlatformPass {
    pub mtl_depth_state: Option<id>
}

#[derive(Default, Clone)]
pub struct MultiMetalBuffer {
    pub buffer: Option<id>,
    pub size: usize,
    pub used: usize
}

#[derive(Default, Clone)]
pub struct MetalBuffer {
    pub last_written: usize,
    pub multi1: MultiMetalBuffer,
    pub multi2: MultiMetalBuffer,
    pub multi3: MultiMetalBuffer,
    pub multi4: MultiMetalBuffer,
    pub multi5: MultiMetalBuffer,
    pub multi6: MultiMetalBuffer,
    pub multi7: MultiMetalBuffer,
    pub multi8: MultiMetalBuffer,
    pub multi9: MultiMetalBuffer,
    pub multi10: MultiMetalBuffer,
}

impl MetalBuffer {
    pub fn multi_buffer_read(&self) -> &MultiMetalBuffer {
        match self.last_written {
            0 => &self.multi1,
            1 => &self.multi2,
            2 => &self.multi3,
            3 => &self.multi4,
            _ => &self.multi5,
        }
    }
    
    pub fn multi_buffer_write(&mut self) -> &mut MultiMetalBuffer {
        self.last_written = (self.last_written + 1) % 5;
        match self.last_written {
            0 => &mut self.multi1,
            1 => &mut self.multi2,
            2 => &mut self.multi3,
            3 => &mut self.multi4,
            _ => &mut self.multi5,
        }
    }
    
    pub fn update_with_f32_data(&mut self, metal_cx: &MetalCx, data: &Vec<f32>) {
        let elem = self.multi_buffer_write();
        if elem.size < data.len() {
            elem.buffer = None;
        }
        if let None = elem.buffer {
            let buffer: id = unsafe {msg_send![
                metal_cx.device,
                newBufferWithLength: (data.len() * std::mem::size_of::<f32>()) as u64
                options: MTLResourceOptions::StorageModeShared
            ]};
            if buffer == nil {elem.buffer = None} else {elem.buffer = Some(buffer)}
            elem.size = data.len()
        }
        
        if let Some(buffer) = elem.buffer {
            unsafe {
                let p: *mut std::ffi::c_void = msg_send![buffer, contents];
                std::ptr::copy(data.as_ptr(), p as *mut f32, data.len());
                let () = msg_send![
                    buffer,
                    didModifyRange: NSRange {
                        location: 0,
                        length: (data.len() * std::mem::size_of::<f32>()) as u64
                    }
                ];
            }
        }
        elem.used = data.len()
    }
    
    pub fn update_with_u32_data(&mut self, metal_cx: &MetalCx, data: &Vec<u32>) {
        let elem = self.multi_buffer_write();
        if elem.size < data.len() {
            elem.buffer = None;
        }
        if let None = elem.buffer {
            let buffer: id = unsafe {msg_send![
                metal_cx.device,
                newBufferWithLength: (data.len() * std::mem::size_of::<u32>()) as u64
                options: MTLResourceOptions::StorageModeShared
            ]};
            if buffer == nil {elem.buffer = None} else {elem.buffer = Some(buffer)}
            elem.size = data.len()
        }
        if let Some(buffer) = elem.buffer {
            unsafe {
                let p: *mut std::ffi::c_void = msg_send![buffer, contents];
                std::ptr::copy(data.as_ptr(), p as *mut u32, data.len());
                let () = msg_send![
                    buffer,
                    didModifyRange: NSRange {
                        location: 0,
                        length: (data.len() * std::mem::size_of::<f32>()) as u64
                    }
                ];
            }
        }
        elem.used = data.len()
    }
}
