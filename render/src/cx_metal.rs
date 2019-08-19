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
    
    pub fn render_view(&mut self, pass_id: usize, view_id: usize, metal_cx: &MetalCx, encoder: &RenderCommandEncoderRef) {
        
        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
        let draw_calls_len = self.views[view_id].draw_calls_len;
        self.views[view_id].set_clipping_uniforms();
        self.views[view_id].uniform_view_transform(&Mat4::identity());
        for draw_call_id in 0..draw_calls_len {
            let sub_view_id = self.views[view_id].draw_calls[draw_call_id].sub_view_id;
            if sub_view_id != 0 {
                self.render_view(pass_id, sub_view_id, metal_cx, encoder);
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
                    if let Some(mtltex) = &cxtexture.platform.mtltexture {
                        encoder.set_fragment_texture(i as NSUInteger, Some(&mtltex));
                        encoder.set_vertex_texture(i as NSUInteger, Some(&mtltex));
                    }
                }
                
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
    
    pub fn draw_pass_to_layer(
        &mut self,
        pass_id: usize,
        _dpi_factor: f32,
        layer: &CoreAnimationLayer,
        metal_cx: &mut MetalCx,
    ) {
        self.platform.bytes_written = 0;
        let view_id = self.passes[pass_id].main_view_id.unwrap();
        let pass_size = self.passes[pass_id].pass_size;
        self.passes[pass_id].set_ortho_matrix(Vec2::zero(), pass_size);
        self.passes[pass_id].uniform_camera_view(&Mat4::identity());
        let pool = unsafe {NSAutoreleasePool::new(cocoa::base::nil)};
        //let command_buffer = command_queue.new_command_buffer();
        if let Some(drawable) = layer.next_drawable() {
            
            let render_pass_descriptor = RenderPassDescriptor::new();
            
            if self.passes[pass_id].color_textures.len()>0 {
                // TODO add z-buffer attachments and multisample attachments
                let color_texture = &self.passes[pass_id].color_textures[0];
                let color_attachment = render_pass_descriptor.color_attachments().object_at(0).unwrap();
                color_attachment.set_texture(Some(drawable.texture()));
                color_attachment.set_store_action(MTLStoreAction::Store);
                if let Some(color) = color_texture.clear_color {
                    color_attachment.set_load_action(MTLLoadAction::Clear);
                    color_attachment.set_clear_color(MTLClearColor::new(color.r as f64, color.g as f64, color.b as f64, color.a as f64));
                }
                else {
                    color_attachment.set_load_action(MTLLoadAction::Load);
                }
            }
            else {
                let color_attachment = render_pass_descriptor.color_attachments().object_at(0).unwrap();
                color_attachment.set_texture(Some(drawable.texture()));
                color_attachment.set_store_action(MTLStoreAction::Store);
                color_attachment.set_load_action(MTLLoadAction::Clear);
                color_attachment.set_clear_color(MTLClearColor::new(0.0, 0.0, 0.0, 0.0))
            }
            
            let command_buffer = metal_cx.command_queue.new_command_buffer();
            let encoder = command_buffer.new_render_command_encoder(&render_pass_descriptor);
            
            self.render_view(pass_id, view_id, &metal_cx, encoder);
            encoder.end_encoding();
            command_buffer.present_drawable(&drawable);
            command_buffer.commit();
            //command_buffer.wait_until_scheduled();
        }
        unsafe {
            msg_send![pool, release];
        }
    }
    
    pub fn draw_pass_to_texture(
        &mut self,
        pass_id: usize,
        dpi_factor: f32,
        metal_cx: &MetalCx,
    ) {
        let view_id = self.passes[pass_id].main_view_id.unwrap();
        let pass_size = self.passes[pass_id].pass_size;
        self.passes[pass_id].set_ortho_matrix(Vec2::zero(), pass_size);
        self.passes[pass_id].uniform_camera_view(&Mat4::identity());

        let pool = unsafe {NSAutoreleasePool::new(cocoa::base::nil)};
        
        let render_pass_descriptor = RenderPassDescriptor::new();
        
        for (index, color_texture) in self.passes[pass_id].color_textures.iter().enumerate() {
            
            let cxtexture = &mut self.textures[color_texture.texture_id];
            
            metal_cx.update_platform_render_target(cxtexture, dpi_factor, pass_size, false);
            let color_attachment = render_pass_descriptor.color_attachments().object_at(index).unwrap();
            if let Some(mtltex) = &cxtexture.platform.mtltexture {
                color_attachment.set_texture(Some(&mtltex));
            }
            else {
                println!("draw_pass_to_texture invalid render target");
            }
            color_attachment.set_store_action(MTLStoreAction::Store);
            if let Some(color) = color_texture.clear_color {
                color_attachment.set_load_action(MTLLoadAction::Clear);
                color_attachment.set_clear_color(MTLClearColor::new(color.r as f64, color.g as f64, color.b as f64, color.a as f64));
            }
            else {
                color_attachment.set_load_action(MTLLoadAction::Load);
            }
        }
        // lets loop and connect all color_textures to our render pass descriptors
        // initializing/allocating/reallocating them to the right size if need be.
        
        let command_buffer = metal_cx.command_queue.new_command_buffer();
        let encoder = command_buffer.new_render_command_encoder(&render_pass_descriptor);
        self.render_view(pass_id, view_id, &metal_cx, encoder);
        encoder.end_encoding();
        
        command_buffer.commit();
        
        unsafe {msg_send![pool, release];}
    }
}

pub struct MetalCx {
    pub device: Device,
    pub command_queue: CommandQueue
}

impl MetalCx {
    
    pub fn new() -> MetalCx {
        let device = Device::system_default();
        MetalCx {
            command_queue: device.new_command_queue(),
            device: device
        }
    }
    
    pub fn update_platform_render_target(&self, cxtexture: &mut CxTexture, dpi_factor: f32, size: Vec2, is_depth: bool) {
        
        let width = if let Some(width) = cxtexture.desc.width {width as u64} else {(size.x * dpi_factor) as u64};
        let height = if let Some(height) = cxtexture.desc.height {height as u64} else {(size.y * dpi_factor) as u64};
        
        if cxtexture.platform.width == width && cxtexture.platform.height == height && cxtexture.platform.alloc_desc == cxtexture.desc {
            return
        }
        cxtexture.platform.mtltexture = None;
        
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
                    return;
                }
            }
        }
        else {
            match cxtexture.desc.format {
                TextureFormat::Default | TextureFormat::Depth24Stencil8 => {
                    mdesc.set_pixel_format(MTLPixelFormat::Depth24Unorm_Stencil8);
                    mdesc.set_texture_type(MTLTextureType::D2);
                    mdesc.set_storage_mode(MTLStorageMode::Private);
                    mdesc.set_usage(MTLTextureUsage::RenderTarget);
                },
                _ => {
                    println!("update_platform_render_targete unsupported texture format");
                    return;
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
        cxtexture.platform.mtltexture = Some(tex);
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
            cxtexture.platform.mtltexture = None;
            let mdesc = TextureDescriptor::new();
            mdesc.set_texture_type(MTLTextureType::D2);
            mdesc.set_width(width as u64);
            mdesc.set_height(height as u64);
            mdesc.set_storage_mode(MTLStorageMode::Managed);
            
            match cxtexture.desc.format {
                TextureFormat::Default | TextureFormat::ImageBGRA => {
                    mdesc.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
                    let tex = self.device.new_texture(&mdesc);
                    cxtexture.platform.mtltexture = Some(tex);
                    
                    if cxtexture.image_u32.len() != width * height {
                        println!("update_platform_texture_image2d with wrong buffer_u32 size!");
                        cxtexture.platform.mtltexture = None;
                        return;
                    }
                    let region = MTLRegion {
                        origin: MTLOrigin {x: 0, y: 0, z: 0},
                        size: MTLSize {width: width as u64, height: height as u64, depth: 1}
                    };
                    if let Some(mtltexture) = &cxtexture.platform.mtltexture {
                        mtltexture.replace_region(
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
            msg_send![core_animation_layer, setMaximumDrawableCount: count];
            msg_send![core_animation_layer, setDisplaySyncEnabled: false];
            msg_send![core_animation_layer, setNeedsDisplayOnBoundsChange: true];
            msg_send![core_animation_layer, setAutoresizingMask: (1 << 4) | (1 << 1)];
            msg_send![core_animation_layer, setAllowsNextDrawableTimeout: false];
            msg_send![core_animation_layer, setDelegate: cocoa_window.view];
            msg_send![core_animation_layer, setBackgroundColor: CGColor::rgb(0.0, 0.0, 0.0, 1.0)];
        }
        
        unsafe {
            let view = cocoa_window.view;
            view.setWantsBestResolutionOpenGLSurface_(YES);
            view.setWantsLayer(YES);
            msg_send![view, setLayerContentsPlacement: 11];
            view.setLayer(mem::transmute(core_animation_layer.as_ref()));
        }
        
        MetalWindow {
            window_id,
            cal_size: Vec2::zero(),
            core_animation_layer,
            window_geom: cocoa_window.get_window_geom(),
            cocoa_window
        }
    }
    
    pub fn set_vsync_enable(&mut self, enable: bool) {
        unsafe {
            msg_send![self.core_animation_layer, setDisplaySyncEnabled: enable];
        }
    }
    
    pub fn set_buffer_count(&mut self, count: u64) {
        unsafe {
            msg_send![self.core_animation_layer, setMaximumDrawableCount: count];
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
pub struct PlatformDrawCall {
    //pub uni_dr: MetalBuffer,
    pub inst_vbuf: MetalBuffer
}

#[derive(Default, Clone, Debug)]
pub struct CxPlatformTexture {
    pub alloc_desc: TextureDesc,
    pub width: u64,
    pub height: u64,
    pub mtltexture: Option<metal::Texture>
}

#[derive(Default, Clone, Debug)]
pub struct CxPlatformPass {
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

use closefds::*;
use std::process::{Command, Child, Stdio};
use std::os::unix::process::{CommandExt};

pub fn spawn_process_command(cmd: &str, args: &[&str], current_dir: &str) -> Result<Child, std::io::Error> {
    unsafe {Command::new(cmd) .args(args) .pre_exec(close_fds_on_exec(vec![0, 1, 2]).unwrap()) .stdout(Stdio::piped()) .stderr(Stdio::piped()) .current_dir(current_dir) .spawn()}
}
