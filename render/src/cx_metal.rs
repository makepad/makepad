use std::mem;

//use cocoa::base::{id};
use cocoa::appkit::{NSView};
use cocoa::foundation::{NSAutoreleasePool, NSUInteger, NSRange};
use core_graphics::geometry::CGSize;
use core_graphics::color::CGColor;
use objc::{msg_send, sel, sel_impl};
use objc::runtime::YES;
use metal::*;
use crate::cx_cocoa::*;
use crate::cx::*;

impl Cx {
    
    pub fn render_view(&mut self, pass_id: usize, view_id: usize, metal_cx: &MetalCx, zbias:&mut f32, zbias_step:f32, encoder: &RenderCommandEncoderRef) {
        
        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
        let draw_calls_len = self.views[view_id].draw_calls_len;
        self.views[view_id].set_clipping_uniforms();
        self.views[view_id].uniform_view_transform(&Mat4::identity());
        for draw_call_id in 0..draw_calls_len {
            let sub_view_id = self.views[view_id].draw_calls[draw_call_id].sub_view_id;
            if sub_view_id != 0 {
                self.render_view(pass_id, sub_view_id, metal_cx, zbias, zbias_step, encoder);
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
                if  draw_call.uniforms.len() > 0{
                    if let Some(zbias_offset) = sh.mapping.zbias_uniform_prop{
                        draw_call.uniforms[zbias_offset] = *zbias;
                        *zbias += zbias_step;
                    }
                }
                
                if draw_call.uniforms_dirty {
                    draw_call.uniforms_dirty = false;
                    //draw_call.platform.uni_dr.update_with_f32_data(device, &draw_call.uniforms);
                }
                
                // lets verify our instance_offset is not disaligned
                let instances = (draw_call.instance.len() / sh.mapping.instance_slots) as u64;
                let pipeline_state = &shp.pipeline_state;
                encoder.set_render_pipeline_state(pipeline_state);
                if let Some(buf) = &shp.geom_vbuf.multi_buffer_read().buffer {encoder.set_vertex_buffer(0, Some(&buf), 0);}
                else {println!("Drawing error: geom_vbuf None")}
                
                if let Some(buf) = &draw_call.platform.inst_vbuf.multi_buffer_read().buffer {encoder.set_vertex_buffer(1, Some(&buf), 0);}
                else {println!("Drawing error: inst_vbuf None")}
                
                let cxuniforms = &self.passes[pass_id].uniforms;
                
                encoder.set_vertex_bytes(2, (cxuniforms.len() * 4) as u64, cxuniforms.as_ptr() as *const std::ffi::c_void);
                encoder.set_vertex_bytes(3, (cxview.uniforms.len() * 4) as u64, cxview.uniforms.as_ptr() as *const std::ffi::c_void);
                encoder.set_vertex_bytes(4, (draw_call.uniforms.len() * 4) as u64, draw_call.uniforms.as_ptr() as *const std::ffi::c_void);
                encoder.set_fragment_bytes(0, (cxuniforms.len() * 4) as u64, cxuniforms.as_ptr() as *const std::ffi::c_void);
                encoder.set_fragment_bytes(1, (cxview.uniforms.len() * 4) as u64, cxview.uniforms.as_ptr() as *const std::ffi::c_void);
                encoder.set_fragment_bytes(2, (draw_call.uniforms.len() * 4) as u64, draw_call.uniforms.as_ptr() as *const std::ffi::c_void);
                
                // lets set our textures
                for (i, texture_id) in draw_call.textures_2d.iter().enumerate() {
                    let cxtexture = &mut self.textures[*texture_id as usize];
                    if cxtexture.update_image {
                        metal_cx.update_platform_texture_image2d(cxtexture);
                    }
                    if let Some(mtl_texture) = &cxtexture.platform.mtl_texture {
                        encoder.set_fragment_texture(i as NSUInteger, Some(&mtl_texture));
                        encoder.set_vertex_texture(i as NSUInteger, Some(&mtl_texture));
                    }
                }
                self.platform.draw_calls_done += 1;
                if let Some(buf) = &shp.geom_ibuf.multi_buffer_read().buffer {
                    encoder.draw_indexed_primitives_instanced(
                        MTLPrimitiveType::Triangle,
                        sh.shader_gen.geometry_indices.len() as u64,
                        // Index Count
                        MTLIndexType::UInt32,
                        // indexType,
                        &buf,
                        // index buffer
                        0,
                        // index buffer offset
                        instances,
                        // instance count
                    )
                }
                else {println!("Drawing error: geom_ibuf None")}
            }
        }
    }
    
    pub fn setup_render_pass_descriptor(&mut self, render_pass_descriptor: &RenderPassDescriptorRef, pass_id: usize, inherit_dpi_factor: f32, first_texture: Option<&metal::TextureRef>, metal_cx: &MetalCx) {
        let pass_size = self.passes[pass_id].pass_size;
        
        self.passes[pass_id].set_ortho_matrix(Vec2::zero(), pass_size);
        self.passes[pass_id].uniform_camera_view(&Mat4::identity());
        self.passes[pass_id].paint_dirty = false;
        let dpi_factor = if let Some(override_dpi_factor) = self.passes[pass_id].override_dpi_factor{
            override_dpi_factor
        }
        else{
            inherit_dpi_factor
        };
        self.passes[pass_id].set_dpi_factor(dpi_factor);
        
        for (index, color_texture) in self.passes[pass_id].color_textures.iter().enumerate() {
            
            let color_attachment = render_pass_descriptor.color_attachments().object_at(0).unwrap();
            
            let is_initial;
            if index == 0 && first_texture.is_some() {
                color_attachment.set_texture(Some(&first_texture.unwrap()));
                is_initial = true;
            }
            else {
                let cxtexture = &mut self.textures[color_texture.texture_id];
                is_initial = metal_cx.update_platform_render_target(cxtexture, dpi_factor, pass_size, false);
                
                if let Some(mtl_texture) = &cxtexture.platform.mtl_texture {
                    color_attachment.set_texture(Some(&mtl_texture));
                }
                else {
                    println!("draw_pass_to_texture invalid render target");
                }
                
            }
            
            color_attachment.set_store_action(MTLStoreAction::Store);
            
            match color_texture.clear_color{
                ClearColor::InitWith(color)=>{
                    if is_initial{
                        color_attachment.set_load_action(MTLLoadAction::Clear);
                        color_attachment.set_clear_color(MTLClearColor::new(color.r as f64, color.g as f64, color.b as f64, color.a as f64));
                    }
                    else{
                        color_attachment.set_load_action(MTLLoadAction::Load);
                    }
                },
                ClearColor::ClearWith(color)=>{
                    color_attachment.set_load_action(MTLLoadAction::Clear);
                    color_attachment.set_clear_color(MTLClearColor::new(color.r as f64, color.g as f64, color.b as f64, color.a as f64));
                }
            }
        }
        // attach depth texture
        if let Some(depth_texture_id) = self.passes[pass_id].depth_texture {
            let cxtexture = &mut self.textures[depth_texture_id];
            let is_initial = metal_cx.update_platform_render_target(cxtexture, dpi_factor, pass_size, true);
            let depth_attachment = render_pass_descriptor.depth_attachment().unwrap();
            if let Some(mtl_texture) = &cxtexture.platform.mtl_texture {
                depth_attachment.set_texture(Some(&mtl_texture));
            }
            else {
                println!("draw_pass_to_texture invalid render target");
            }
            depth_attachment.set_store_action(MTLStoreAction::Store);
            
            match self.passes[pass_id].clear_depth{
                ClearDepth::InitWith(depth)=>{
                    if is_initial{
                        depth_attachment.set_load_action(MTLLoadAction::Clear);
                        depth_attachment.set_clear_depth(depth);
                    }
                    else {
                        depth_attachment.set_load_action(MTLLoadAction::Load);
                    }
                },
                ClearDepth::ClearWith(depth)=>{
                    depth_attachment.set_load_action(MTLLoadAction::Clear);
                    depth_attachment.set_clear_depth(depth);
                }
            }
            // create depth state
            if self.passes[pass_id].platform.mtl_depth_state.is_none() {
                let desc = DepthStencilDescriptor::new();
                desc.set_depth_compare_function(MTLCompareFunction::LessEqual);
                desc.set_depth_write_enabled(true);
                self.passes[pass_id].platform.mtl_depth_state = Some(metal_cx.device.new_depth_stencil_state(&desc));
            }
        }
    }
    
    pub fn draw_pass_to_layer(
        &mut self,
        pass_id: usize,
        dpi_factor: f32,
        layer: &CoreAnimationLayer,
        metal_cx: &mut MetalCx,
    ) {
        self.platform.bytes_written = 0;
        self.platform.draw_calls_done = 0;
        let view_id = self.passes[pass_id].main_view_id.unwrap();
        let pool = unsafe {NSAutoreleasePool::new(cocoa::base::nil)};
        //let command_buffer = command_queue.new_command_buffer();
        if let Some(drawable) = layer.next_drawable() {
            let render_pass_descriptor = RenderPassDescriptor::new();
            
            self.setup_render_pass_descriptor(&render_pass_descriptor, pass_id, dpi_factor, Some(drawable.texture()), metal_cx);
            
            let command_buffer = metal_cx.command_queue.new_command_buffer();
            let encoder = command_buffer.new_render_command_encoder(&render_pass_descriptor);
            if let Some(depth_state) = &self.passes[pass_id].platform.mtl_depth_state {
                encoder.set_depth_stencil_state(depth_state);
            }
            let mut zbias = 0.0;
            let zbias_step = self.passes[pass_id].zbias_step;
            self.render_view(pass_id, view_id, &metal_cx, &mut zbias, zbias_step, encoder);
            
            encoder.end_encoding();
            command_buffer.present_drawable(&drawable);
            command_buffer.commit();
            //command_buffer.wait_until_scheduled();
        }
        unsafe {
            let () = msg_send![pool, release];
        }
    }
    
    pub fn draw_pass_to_texture(
        &mut self,
        pass_id: usize,
        dpi_factor: f32,
        metal_cx: &MetalCx,
    ) {
        let view_id = self.passes[pass_id].main_view_id.unwrap();
        
        let pool = unsafe {NSAutoreleasePool::new(cocoa::base::nil)};
        let render_pass_descriptor = RenderPassDescriptor::new();
        
        self.setup_render_pass_descriptor(&render_pass_descriptor, pass_id, dpi_factor, None, metal_cx);
        
        let command_buffer = metal_cx.command_queue.new_command_buffer();
        let encoder = command_buffer.new_render_command_encoder(&render_pass_descriptor);
        if let Some(depth_state) = &self.passes[pass_id].platform.mtl_depth_state {
            encoder.set_depth_stencil_state(depth_state);
        }
        let mut zbias = 0.0;
        let zbias_step = self.passes[pass_id].zbias_step;
        self.render_view(pass_id, view_id, &metal_cx, &mut zbias, zbias_step, encoder);
        
        encoder.end_encoding();
        command_buffer.commit();
        
        unsafe {let () = msg_send![pool, release];}
    }
}

pub struct MetalCx {
    pub device: Device,
    pub command_queue: CommandQueue
}

impl MetalCx {
    
    pub fn new() -> MetalCx {
        let devices = Device::all();
        for device in devices {
            if device.is_low_power() {
                return MetalCx {
                    command_queue: device.new_command_queue(),
                    device: device
                }
            }
        }
        let device = Device::system_default().unwrap();
        MetalCx {
            command_queue: device.new_command_queue(),
            device: device
        }
    }
    
    pub fn update_platform_render_target(&self, cxtexture: &mut CxTexture, dpi_factor: f32, size: Vec2, is_depth: bool)->bool {
        
        let width = if let Some(width) = cxtexture.desc.width {width as u64} else {(size.x * dpi_factor) as u64};
        let height = if let Some(height) = cxtexture.desc.height {height as u64} else {(size.y * dpi_factor) as u64};
        
        if cxtexture.platform.width == width && cxtexture.platform.height == height && cxtexture.platform.alloc_desc == cxtexture.desc {
            return false
        }
        cxtexture.platform.mtl_texture = None;
        
        let mdesc = TextureDescriptor::new();
        if !is_depth {
            match cxtexture.desc.format {
                TextureFormat::Default | TextureFormat::RenderBGRA => {
                    mdesc.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
                    mdesc.set_texture_type(MTLTextureType::D2);
                    mdesc.set_storage_mode(MTLStorageMode::Private);
                    mdesc.set_usage(MTLTextureUsage::RenderTarget);
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
                    mdesc.set_pixel_format(MTLPixelFormat::Depth32Float_Stencil8);
                    mdesc.set_texture_type(MTLTextureType::D2);
                    mdesc.set_storage_mode(MTLStorageMode::Private);
                    mdesc.set_usage(MTLTextureUsage::RenderTarget);
                },
                _ => {
                    println!("update_platform_render_targete unsupported texture format");
                    return false;
                }
            }
        }
        mdesc.set_width(width as u64);
        mdesc.set_height(height as u64);
        mdesc.set_depth(1);
        let tex = self.device.new_texture(&mdesc);
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
            let mdesc = TextureDescriptor::new();
            mdesc.set_texture_type(MTLTextureType::D2);
            mdesc.set_width(width as u64);
            mdesc.set_height(height as u64);
            mdesc.set_storage_mode(MTLStorageMode::Managed);
            
            match cxtexture.desc.format {
                TextureFormat::Default | TextureFormat::ImageBGRA => {
                    mdesc.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
                    let tex = self.device.new_texture(&mdesc);
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
                    if let Some(mtl_texture) = &cxtexture.platform.mtl_texture {
                        mtl_texture.replace_region(
                            region,
                            0,
                            (width * std::mem::size_of::<u32>()) as u64,
                            cxtexture.image_u32.as_ptr() as *const std::ffi::c_void
                        );
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
    pub core_animation_layer: CoreAnimationLayer,
    pub cocoa_window: CocoaWindow,
}

impl MetalWindow {
    pub fn new(window_id: usize, metal_cx: &MetalCx, cocoa_app: &mut CocoaApp, inner_size: Vec2, position: Option<Vec2>, title: &str) -> MetalWindow {
        
        let core_animation_layer = CoreAnimationLayer::new();
        
        let mut cocoa_window = CocoaWindow::new(cocoa_app, window_id);
        
        cocoa_window.init(title, inner_size, position);
        
        core_animation_layer.set_device(&metal_cx.device);
        core_animation_layer.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
        core_animation_layer.set_presents_with_transaction(false);
        
        unsafe {
            //msg_send![layer, displaySyncEnabled:false];
            let count: u64 = 2;
            let () = msg_send![core_animation_layer, setMaximumDrawableCount: count];
            let () = msg_send![core_animation_layer, setDisplaySyncEnabled: false];
            let () = msg_send![core_animation_layer, setNeedsDisplayOnBoundsChange: true];
            let () = msg_send![core_animation_layer, setAutoresizingMask: (1 << 4) | (1 << 1)];
            let () = msg_send![core_animation_layer, setAllowsNextDrawableTimeout: false];
            let () = msg_send![core_animation_layer, setDelegate: cocoa_window.view];
            let () = msg_send![core_animation_layer, setBackgroundColor: CGColor::rgb(0.0, 0.0, 0.0, 1.0)];
        }
        
        unsafe {
            let view = cocoa_window.view;
            view.setWantsBestResolutionOpenGLSurface_(YES);
            view.setWantsLayer(YES);
            let () = msg_send![view, setLayerContentsPlacement: 11];
            view.setLayer(mem::transmute(core_animation_layer.as_ref()));
        }
        
        MetalWindow {
            first_draw:true,
            window_id,
            cal_size: Vec2::zero(),
            core_animation_layer,
            window_geom: cocoa_window.get_window_geom(),
            cocoa_window
        }
    }
    
    pub fn set_vsync_enable(&mut self, enable: bool) {
        unsafe {
            let () = msg_send![self.core_animation_layer, setDisplaySyncEnabled: enable];
        }
    }
    
    pub fn set_buffer_count(&mut self, count: u64) {
        unsafe {
            let () = msg_send![self.core_animation_layer, setMaximumDrawableCount: count];
        }
    }
    
    pub fn resize_core_animation_layer(&mut self, _metal_cx: &MetalCx) -> bool {
        let cal_size = Vec2 {
            x: self.window_geom.inner_size.x * self.window_geom.dpi_factor,
            y: self.window_geom.inner_size.y * self.window_geom.dpi_factor
        };
        if self.cal_size != cal_size {
            self.cal_size = cal_size;
            self.core_animation_layer.set_drawable_size(CGSize::new(cal_size.x as f64, cal_size.y as f64));
            self.core_animation_layer.set_contents_scale(self.window_geom.dpi_factor as f64);
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

#[derive(Default, Clone, Debug)]
pub struct CxPlatformDrawCall {
    //pub uni_dr: MetalBuffer,
    pub inst_vbuf: MetalBuffer
}

#[derive(Default, Clone, Debug)]
pub struct CxPlatformTexture {
    pub alloc_desc: TextureDesc,
    pub width: u64,
    pub height: u64,
    pub mtl_texture: Option<metal::Texture>
}

#[derive(Default, Clone, Debug)]
pub struct CxPlatformPass {
    pub mtl_depth_state: Option<metal::DepthStencilState>
}

#[derive(Default, Clone, Debug)]
pub struct MultiMetalBuffer {
    pub buffer: Option<metal::Buffer>,
    pub size: usize,
    pub used: usize
}

#[derive(Default, Clone, Debug)]
pub struct MetalBuffer {
    pub last_written: usize,
    pub multi1: MultiMetalBuffer,
    pub multi2: MultiMetalBuffer,
    pub multi3: MultiMetalBuffer,
}

impl MetalBuffer {
    pub fn multi_buffer_read(&self) -> &MultiMetalBuffer {
        match self.last_written {
            0 => &self.multi1,
            1 => &self.multi2,
            _ => &self.multi3,
        }
    }
    
    pub fn multi_buffer_write(&mut self) -> &mut MultiMetalBuffer {
        self.last_written = (self.last_written + 1) % 3;
        match self.last_written {
            0 => &mut self.multi1,
            1 => &mut self.multi2,
            _ => &mut self.multi3,
        }
    }
    
    pub fn update_with_f32_data(&mut self, metal_cx: &MetalCx, data: &Vec<f32>) {
        let elem = self.multi_buffer_write();
        if elem.size < data.len() {
            elem.buffer = None;
        }
        if let None = &elem.buffer {
            elem.buffer = Some(
                metal_cx.device.new_buffer(
                    (data.len() * std::mem::size_of::<f32>()) as u64,
                    MTLResourceOptions::CPUCacheModeDefaultCache
                )
            );
            elem.size = data.len()
        }
        if let Some(buffer) = &elem.buffer {
            let p = buffer.contents();
            
            unsafe {
                std::ptr::copy(data.as_ptr(), p as *mut f32, data.len());
            }
            buffer.did_modify_range(NSRange::new(0 as u64, (data.len() * std::mem::size_of::<f32>()) as u64));
        }
        elem.used = data.len()
    }
    
    pub fn update_with_u32_data(&mut self, metal_cx: &MetalCx, data: &Vec<u32>) {
        let elem = self.multi_buffer_write();
        if elem.size < data.len() {
            elem.buffer = None;
        }
        if let None = &elem.buffer {
            elem.buffer = Some(
                metal_cx.device.new_buffer(
                    (data.len() * std::mem::size_of::<u32>()) as u64,
                    MTLResourceOptions::CPUCacheModeDefaultCache
                )
            );
            elem.size = data.len()
        }
        if let Some(buffer) = &elem.buffer {
            let p = buffer.contents();
            
            unsafe {
                std::ptr::copy(data.as_ptr(), p as *mut u32, data.len());
            }
            buffer.did_modify_range(NSRange::new(0 as u64, (data.len() * std::mem::size_of::<u32>()) as u64));
        }
        elem.used = data.len()
    }
}