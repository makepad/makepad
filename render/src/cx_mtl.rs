use std::mem;

//use cocoa::base::{id};
use cocoa::appkit::{NSView};
use cocoa::foundation::{NSAutoreleasePool, NSUInteger, NSRange};
use core_graphics::geometry::CGSize;
use objc::{msg_send, sel, sel_impl};
use objc::runtime::YES;
use metal::*;
use crate::cx_cocoa::*;
use crate::cx::*;

impl Cx {
    
    pub fn render_view(&mut self, pass_id: usize, view_id: usize, metal_cx: &MetalCx, encoder: &RenderCommandEncoderRef) {
        
        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
        let draw_calls_len = self.views[view_id].draw_calls_len;
        for draw_call_id in 0..draw_calls_len {
            let sub_view_id = self.views[view_id].draw_calls[draw_call_id].sub_view_id;
            if sub_view_id != 0 {
                self.render_view(pass_id, sub_view_id, metal_cx, encoder);
            }
            else {
                let cxview = &mut self.views[view_id];
                cxview.set_clipping_uniforms();
                //view.platform.uni_vw.update_with_f32_data(device, &view.uniforms);
                let draw_call = &mut cxview.draw_calls[draw_call_id];
                let sh = &self.shaders[draw_call.shader_id];
                let shp = sh.platform.as_ref().unwrap();
                
                if draw_call.instance_dirty {
                    draw_call.instance_dirty = false;
                    // update the instance buffer data
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
                    if cxtexture.upload_image {
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
        metal_cx: &MetalCx,
    ) {
        let view_id = self.passes[pass_id].main_view_id.unwrap();
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
    
    pub fn event_loop<F>(&mut self, mut event_handler: F)
    where F: FnMut(&mut Cx, &mut Event),
    {
        self.is_desktop_build = true;
        
        let mut cocoa_app = CocoaApp::new();
        
        cocoa_app.init();
        
        let metal_cx = MetalCx::new();
        
        let mut render_windows: Vec<CocoaRenderWindow> = Vec::new();
        
        self.mtl_compile_all_shaders(&metal_cx);
        
        self.load_fonts_from_file();
        
        self.call_event_handler(&mut event_handler, &mut Event::Construct);
        
        self.redraw_child_area(Area::All);
        
        let mut passes_todo = Vec::new();
        
        cocoa_app.event_loop( | cocoa_app, events | {
            let mut paint_dirty = false;
            for mut event in events {
                
                self.process_desktop_pre_event(&mut event, &mut event_handler);
                
                match &event {
                    Event::WindowGeomChange(re) => { // do this here because mac
                        for render_window in &mut render_windows {
                            if render_window.window_id == re.window_id {
                                render_window.window_geom = re.new_geom.clone();
                                self.windows[re.window_id].window_geom = re.new_geom.clone();
                                // redraw just this windows root draw list
                                if let Some(main_pass_id) = self.windows[re.window_id].main_pass_id {
                                    self.redraw_pass_and_sub_passes(main_pass_id);
                                }
                                break;
                            }
                        }
                        // ok lets not redraw all, just this window
                        self.call_event_handler(&mut event_handler, &mut event);
                    },
                    Event::Paint => {
                        
                        let vsync = self.process_desktop_paint_callbacks(cocoa_app.time_now(), &mut event_handler);
                        
                        // construct or destruct windows
                        for (index, window) in self.windows.iter_mut().enumerate() {
                            
                            window.window_state = match &window.window_state {
                                CxWindowState::Create {inner_size, position, title} => {
                                    // lets create a platformwindow
                                    let render_window = CocoaRenderWindow::new(index, &metal_cx, cocoa_app, *inner_size, *position, &title);
                                    window.window_geom = render_window.window_geom.clone();
                                    render_windows.push(render_window);
                                    for render_window in &mut render_windows {
                                        render_window.cocoa_window.update_ptrs();
                                    }
                                    CxWindowState::Created
                                },
                                CxWindowState::Destroy => {
                                    CxWindowState::Destroyed
                                },
                                CxWindowState::Created => CxWindowState::Created,
                                CxWindowState::Destroyed => CxWindowState::Destroyed
                            }
                        }
                        
                        // set a cursor
                        if !self.down_mouse_cursor.is_none() {
                            cocoa_app.set_mouse_cursor(self.down_mouse_cursor.as_ref().unwrap().clone())
                        }
                        else if !self.hover_mouse_cursor.is_none() {
                            cocoa_app.set_mouse_cursor(self.hover_mouse_cursor.as_ref().unwrap().clone())
                        }
                        else {
                            cocoa_app.set_mouse_cursor(MouseCursor::Default)
                        }
                        
                        if let Some(set_ime_position) = self.platform.set_ime_position {
                            self.platform.set_ime_position = None;
                            for render_window in &mut render_windows {
                                render_window.cocoa_window.set_ime_spot(set_ime_position);
                            }
                        }
                        
                        while self.platform.start_timer.len() > 0 {
                            let (timer_id, interval, repeats) = self.platform.start_timer.pop().unwrap();
                            cocoa_app.start_timer(timer_id, interval, repeats);
                        }
                        
                        while self.platform.stop_timer.len() > 0 {
                            let timer_id = self.platform.stop_timer.pop().unwrap();
                            cocoa_app.stop_timer(timer_id);
                        }
                        
                        // build a list of renderpasses to repaint
                        let mut windows_need_repaint = 0;
                        self.compute_passes_to_repaint(&mut passes_todo, &mut windows_need_repaint);
                        
                        if passes_todo.len() > 0 {
                            for pass_id in &passes_todo {
                                match self.passes[*pass_id].dep_of.clone() {
                                    CxPassDepOf::Window(window_id) => {
                                        // find the accompanying render window
                                        let render_window = render_windows.iter_mut().find( | w | w.window_id == window_id).unwrap();
                                        
                                        // its a render window
                                        windows_need_repaint -= 1;
                                        render_window.set_vsync_enable(windows_need_repaint == 0 && vsync);
                                        render_window.set_buffer_count(
                                            if render_window.window_geom.is_fullscreen {3}else {2}
                                        );
                                        
                                        let dpi_factor = render_window.window_geom.dpi_factor;
                                        self.passes[*pass_id].set_dpi_factor(dpi_factor);
                                        
                                        self.draw_pass_to_layer(
                                            *pass_id,
                                            dpi_factor,
                                            &render_window.core_animation_layer,
                                            &metal_cx,
                                        );
                                        
                                        if render_window.resize_core_animation_layer(&metal_cx) {
                                            self.passes[*pass_id].paint_dirty = true;
                                            paint_dirty = true;
                                        }
                                        else {
                                            self.passes[*pass_id].paint_dirty = false;
                                        }
                                    }
                                    CxPassDepOf::Pass(parent_pass_id) => {
                                        let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                                        self.passes[*pass_id].set_dpi_factor(dpi_factor);
                                        self.draw_pass_to_texture(
                                            *pass_id,
                                            dpi_factor,
                                            &metal_cx,
                                        );
                                    },
                                    CxPassDepOf::None => ()
                                }
                            }
                        }
                    },
                    Event::None => {
                    },
                    _ => {
                        self.call_event_handler(&mut event_handler, &mut event);
                    }
                }
                if self.process_desktop_post_event(event) {
                    cocoa_app.terminate_event_loop();
                }
            }
            if self.playing_anim_areas.len() == 0 && self.redraw_parent_areas.len() == 0 && self.redraw_child_areas.len() == 0 && self.frame_callbacks.len() == 0 && !paint_dirty {
                true
            } else {
                false
            }
        })
    }
    
    pub fn show_text_ime(&mut self, x: f32, y: f32) {
        self.platform.set_ime_position = Some(Vec2 {x: x, y: y});
    }
    
    pub fn hide_text_ime(&mut self) {
    }
    
    pub fn set_window_outer_size(&mut self, size: Vec2) {
        self.platform.set_window_outer_size = Some(size);
    }
    
    pub fn set_window_position(&mut self, pos: Vec2) {
        self.platform.set_window_position = Some(pos);
    }
    
    pub fn start_timer(&mut self, interval: f64, repeats: bool) -> Timer {
        self.timer_id += 1;
        self.platform.start_timer.push((self.timer_id, interval, repeats));
        Timer {timer_id: self.timer_id}
    }
    
    pub fn stop_timer(&mut self, timer: &mut Timer) {
        if timer.timer_id != 0 {
            self.platform.stop_timer.push(timer.timer_id);
            timer.timer_id = 0;
        }
    }
    
    pub fn send_signal(signal: Signal, value: u64) {
        CocoaApp::post_signal(signal.signal_id, value);
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
        
        cxtexture.upload_image = false;
    }
}


#[derive(Clone, Default)]
pub struct CxPlatform {
    pub set_window_position: Option<Vec2>,
    pub set_window_outer_size: Option<Vec2>,
    pub set_ime_position: Option<Vec2>,
    pub start_timer: Vec<(u64, f64, bool)>,
    pub stop_timer: Vec<(u64)>,
    pub text_clipboard_response: Option<String>,
    pub desktop: CxDesktop,
}

#[derive(Clone)]
pub struct CocoaRenderWindow {
    pub window_id: usize,
    pub window_geom: WindowGeom,
    pub cal_size: Vec2,
    pub core_animation_layer: CoreAnimationLayer,
    pub cocoa_window: CocoaWindow,
}

impl CocoaRenderWindow {
    fn new(window_id: usize, metal_cx: &MetalCx, cocoa_app: &mut CocoaApp, inner_size: Vec2, position: Option<Vec2>, title: &str) -> CocoaRenderWindow {
        
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
        }
        
        unsafe {
            let view = cocoa_window.view;
            view.setWantsBestResolutionOpenGLSurface_(YES);
            view.setWantsLayer(YES);
            view.setLayer(mem::transmute(core_animation_layer.as_ref()));
        }
        
        CocoaRenderWindow {
            window_id,
            cal_size: Vec2::zero(),
            core_animation_layer,
            window_geom: cocoa_window.get_window_geom(),
            cocoa_window
        }
    }
    
    fn set_vsync_enable(&mut self, enable: bool) {
        unsafe {
            msg_send![self.core_animation_layer, setDisplaySyncEnabled: enable];
        }
    }
    
    fn set_buffer_count(&mut self, count: u64) {
        unsafe {
            msg_send![self.core_animation_layer, setMaximumDrawableCount: count];
        }
    }
    
    fn resize_core_animation_layer(&mut self, _metal_cx: &MetalCx) -> bool {
        let cal_size = Vec2 {
            x: self.window_geom.inner_size.x * self.window_geom.dpi_factor,
            y: self.window_geom.inner_size.y * self.window_geom.dpi_factor
        };
        if self.cal_size != cal_size {
            self.cal_size = cal_size;
            self.core_animation_layer.set_drawable_size(CGSize::new(cal_size.x as f64, cal_size.y as f64));
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
