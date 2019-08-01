use crate::cx_win32::*;
use crate::cx::*;
use crate::cx_desktop::*;

use winapi::shared::guiddef::GUID;
use winapi::shared::minwindef::{TRUE, FALSE};
use winapi::shared::{dxgi, dxgi1_2, dxgitype, dxgiformat, winerror};
use winapi::um::{d3d11, d3dcommon, d3dcompiler};
use winapi::Interface;
use wio::com::ComPtr;
use std::mem;
use std::ptr;
use std::ffi;
use std::ffi::c_void;
use std::sync::Mutex;

pub struct CxThread {
    cx: u64
}

impl CxThread {
    
    pub fn lock_mapped_texture_f32(&mut self, texture: &Texture, user_data: usize) -> Option<&mut [f32]> {
        if let Some(texture_id) = texture.texture_id {
            let cx = unsafe {&mut *(self.cx as *mut Cx)};
            let cxtexture = &mut cx.textures[texture_id];
            if let Ok(mut mapped) = cxtexture.platform.mapped.lock() {
                let write = mapped.write;
                let texture = &mut mapped.textures[write % MAPPED_TEXTURE_BUFFER_COUNT];
                if let Some(ptr) = texture.map_ptr {
                    texture.user_data = Some(user_data);
                    if texture.locked == true {
                        panic!("lock_mapped_texture_f32 ran twice for the same texture, pair with unlock_mapped_texture!")
                    }
                    texture.locked = true;
                    return Some(unsafe {std::slice::from_raw_parts_mut(
                        ptr as *mut f32,
                        texture.width * texture.height * texture.slots_per_pixel
                    )});
                }
            };
        }
        None
    }
    
    pub fn lock_mapped_texture_u32(&mut self, texture: &Texture, user_data: usize) -> Option<&mut [u32]> {
        if let Some(texture_id) = texture.texture_id {
            let cx = unsafe {&mut *(self.cx as *mut Cx)};
            let cxtexture = &mut cx.textures[texture_id];
            if let Ok(mut mapped) = cxtexture.platform.mapped.lock() {
                let write = mapped.write;
                let texture = &mut mapped.textures[write % MAPPED_TEXTURE_BUFFER_COUNT];
                if let Some(ptr) = texture.map_ptr {
                    texture.user_data = Some(user_data);
                    if texture.locked == true {
                        panic!("lock_mapped_texture_u32 ran twice for the same texture, pair with unlock_mapped_texture!")
                    }
                    texture.locked = true;
                    return Some(unsafe {std::slice::from_raw_parts_mut(
                        ptr as *mut u32,
                        texture.width * texture.height * texture.slots_per_pixel
                    )});
                }
            };
        }
        None
    }
    
    pub fn unlock_mapped_texture(&mut self, texture: &Texture) {
        if let Some(texture_id) = texture.texture_id {
            let cx = unsafe {&mut *(self.cx as *mut Cx)};
            let cxtexture = &mut cx.textures[texture_id];
            if let Ok(mut mapped) = cxtexture.platform.mapped.lock() {
                // lets try to get the texture at mapped.write
                let write = mapped.write;
                let texture = &mut mapped.textures[write % MAPPED_TEXTURE_BUFFER_COUNT];
                if !texture.locked { // no-op
                    return
                }
                texture.locked = false;
                mapped.write += 1; // increase the write pointer
            };
        }
    }
}

impl Cx {
    pub fn post_signal(signal: Signal, value: usize) {
        Win32App::post_signal(signal.signal_id, value);
    }
    
    pub fn new_cxthread(&mut self) -> CxThread {
        CxThread {cx: self as *mut _ as u64}
    }
    
    pub fn get_mapped_texture_user_data(&mut self, texture: &Texture) -> Option<usize> {
        if let Some(texture_id) = texture.texture_id {
            let cxtexture = &self.textures[texture_id];
            if let Ok(mapped) = cxtexture.platform.mapped.lock() {
                return mapped.textures[mapped.read % MAPPED_TEXTURE_BUFFER_COUNT].user_data;
            }
        }
        None
    }
    
    pub fn render_view(&mut self, pass_id: usize, view_id: usize, repaint_id: u64, d3d11_cx: &D3d11Cx) {
        
        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
        let draw_calls_len = {
            let cxview = &mut self.views[view_id];
            cxview.set_clipping_uniforms();
            cxview.platform.uni_dl.update_with_f32_constant_data(d3d11_cx, &mut cxview.uniforms)
            cxview.draw_calls_len
        };
        for draw_call_id in 0..draw_calls_len {
            let sub_view_id = self.views[view_id].draw_calls[draw_call_id].sub_view_id;
            if sub_view_id != 0 {
                self.render_view(pass_id, sub_view_id, repaint_id, d3d11_cx);
            }
            else {
                let cxview = &mut self.views[view_id];
                
                let draw_call = &mut cxview.draw_calls[draw_call_id];
                let sh = &self.shaders[draw_call.shader_id];
                let shp = sh.platform.as_ref().unwrap();
                
                if draw_call.instance_dirty {
                    draw_call.instance_dirty = false;
                    if draw_call.instance.len() == 0 {
                        continue;
                    }
                    // update the instance buffer data
                    draw_call.platform.inst_vbuf.update_with_f32_vertex_data(d3d11_cx, &draw_call.instance);
                }
                if draw_call.uniforms_dirty {
                    draw_call.uniforms_dirty = false;
                    if draw_call.uniforms.len() != 0 {
                        draw_call.platform.uni_dr.update_with_f32_constant_data(d3d11_cx, &mut draw_call.uniforms);
                    }
                }
                
                let instances = (draw_call.instance.len() / sh.mapping.instance_slots) as usize;
                
                if instances == 0 {
                    continue;
                }
                
                d3d11_cx.set_shaders(&shp.vertex_shader, &shp.pixel_shader);
                
                d3d11_cx.set_primitive_topology();
                
                d3d11_cx.set_input_layout(&shp.input_layout);
                
                d3d11_cx.set_index_buffer(&shp.geom_ibuf);
                
                d3d11_cx.set_vertex_buffers(&shp.geom_vbuf, sh.mapping.geometry_slots, &draw_call.platform.inst_vbuf, sh.mapping.instance_slots);
                
                d3d11_cx.set_constant_buffers(&self.platform.uni_cx, &cxview.platform.uni_dl, &draw_call.platform.uni_dr);
                
                for (i, texture_id) in draw_call.textures_2d.iter().enumerate() {
                    let cxtexture = &mut self.textures[*texture_id as usize];
                    match cxtexture.desc.format { // we only allocate Image and Mapped textures.
                        TextureFormat::Default | TextureFormat::ImageBGRA => {
                            if cxtexture.update_image {
                                cxtexture.update_image = false;
                                d3d11_cx.update_platform_texture_image_bgra(
                                    &mut cxtexture.platform.single,
                                    cxtexture.desc.width.unwrap(),
                                    cxtexture.desc.height.unwrap(),
                                    &cxtexture.image_u32,
                                );
                            }
                            d3d11_cx.set_shader_resource(i, &cxtexture.platform.single.shader_resource);
                        },
                        TextureFormat::ImageBGRAf32 => {},
                        TextureFormat::ImageRf32 => {
                            if cxtexture.update_image {
                                cxtexture.update_image = false;
                                d3d11_cx.update_platform_texture_image_rf32(
                                    &mut cxtexture.platform.single,
                                    cxtexture.desc.width.unwrap(),
                                    cxtexture.desc.height.unwrap(),
                                    &cxtexture.image_f32,
                                );
                            }
                            d3d11_cx.set_shader_resource(i, &cxtexture.platform.single.shader_resource);
                        },
                        TextureFormat::ImageRGf32 => {
                            if cxtexture.update_image {
                                cxtexture.update_image = false;
                                d3d11_cx.update_platform_texture_image_rgf32(
                                    &mut cxtexture.platform.single,
                                    cxtexture.desc.width.unwrap(),
                                    cxtexture.desc.height.unwrap(),
                                    &cxtexture.image_f32,
                                );
                            }
                            d3d11_cx.set_shader_resource(i, &cxtexture.platform.single.shader_resource);
                        },
                        TextureFormat::MappedRGf32 | TextureFormat::MappedRf32 | TextureFormat::MappedBGRA | TextureFormat::MappedBGRAf32 => {
                            if let Ok(mut mapped) = cxtexture.platform.mapped.lock() {
                                if mapped.textures[0].texture.is_none() {
                                    d3d11_cx.init_mapped_textures(
                                        &mut *mapped,
                                        &cxtexture.desc.format,
                                        cxtexture.desc.width.unwrap(),
                                        cxtexture.desc.height.unwrap()
                                    );
                                }
                                if let Some(index) = d3d11_cx.flip_mapped_textures(&mut mapped, repaint_id) {
                                    d3d11_cx.set_shader_resource(i, &mapped.textures[index].shader_resource);
                                }
                            }
                        },
                        TextureFormat::RenderBGRA | TextureFormat::RenderBGRAf16 | TextureFormat::RenderBGRAf32 => {
                            d3d11_cx.set_shader_resource(i, &cxtexture.platform.single.shader_resource);
                        },
                        _ => ()
                    }
                }
                d3d11_cx.draw_indexed_instanced(sh.shader_gen.geometry_indices.len(), instances);
            }
        }
    }
    
    fn draw_pass_to_window(&mut self, pass_id: usize, vsync: bool, _dpi_factor: f32, d3d11_window: &mut D3d11Window, d3d11_cx: &D3d11Cx) {
        // let time1 = Cx::profile_time_ns();
        let view_id = self.passes[pass_id].main_view_id.unwrap();
        let pass_size = self.passes[pass_id].pass_size;
        self.passes[pass_id].set_ortho_matrix(Vec2::zero(), pass_size);
        
        self.platform.uni_cx.update_with_f32_constant_data(&d3d11_cx, &mut self.passes[pass_id].uniforms);
        
        let wg = &d3d11_window.window_geom;
        d3d11_cx.set_viewport(wg.inner_size.x * wg.dpi_factor, wg.inner_size.y * wg.dpi_factor);
        d3d11_cx.set_rendertargets(&d3d11_window.render_target);
        d3d11_window.render_target.set_raster_and_blend_state(d3d11_cx);
        
        if self.passes[pass_id].color_textures.len()>0 {
            let color_texture = &self.passes[pass_id].color_textures[0];
            if let Some(color) = color_texture.clear_color {
                d3d11_cx.clear_render_target_view(&d3d11_window.render_target, color); //self.clear_color);
            }
        }
        else {
            d3d11_cx.clear_render_target_view(&d3d11_window.render_target, Color::zero()); //self.clear_color);
        }
        //d3d11_cx.clear_depth_stencil_view(d3d11_window);
        
        self.render_view(pass_id, view_id, self.repaint_id, &d3d11_cx);
        
        d3d11_window.present(vsync);
        //println!("{}", (Cx::profile_time_ns() - time1)as f64 / 1000.0);
    }
    
    fn draw_pass_to_texture(&mut self, pass_id: usize, dpi_factor: f32, d3d11_cx: &D3d11Cx) {
        // let time1 = Cx::profile_time_ns();
        let view_id = self.passes[pass_id].main_view_id.unwrap();
        let pass_size = self.passes[pass_id].pass_size;
        self.passes[pass_id].set_ortho_matrix(Vec2::zero(), pass_size);
        
        self.platform.uni_cx.update_with_f32_constant_data(&d3d11_cx, &mut self.passes[pass_id].uniforms);
        
        //d3d11_cx.set_viewport(d3d11_window);
        
        if self.passes[pass_id].color_textures.len() == 0 {
            return
        }
        
        let color_texture = &mut self.passes[pass_id].color_textures[0];
        let mut cxtexture = &mut self.textures[color_texture.texture_id];
        // allocate/resize render target color texture
        d3d11_cx.update_render_target(&mut cxtexture, dpi_factor, pass_size);
        
        if let Some(render) = &cxtexture.platform.render {
            d3d11_cx.set_rendertargets(render);
            render.set_raster_and_blend_state(d3d11_cx);
            d3d11_cx.set_viewport(cxtexture.platform.single.width as f32, cxtexture.platform.single.height as f32);
            if let Some(color) = color_texture.clear_color {
                d3d11_cx.clear_render_target_view(render, color);
            }
            self.render_view(pass_id, view_id, self.repaint_id, &d3d11_cx);
        }
    }
    
    pub fn event_loop<F>(&mut self, mut event_handler: F)
    where F: FnMut(&mut Cx, &mut Event),
    {
        self.platform_type = PlatformType::Windows;
        
        let mut win32_app = Win32App::new();
        
        win32_app.init();
        
        let mut d3d11_windows: Vec<D3d11Window> = Vec::new();
        
        let d3d11_cx = D3d11Cx::new();
        
        self.platform.d3d11_cx = Some(&d3d11_cx);
        
        self.hlsl_compile_all_shaders(&d3d11_cx);
        
        self.load_fonts_from_file();
        
        self.call_event_handler(&mut event_handler, &mut Event::Construct);
        
        self.redraw_child_area(Area::All);
        let mut passes_todo = Vec::new();
        
        win32_app.event_loop( | win32_app, events | { //if let Ok(d3d11_cx) = d3d11_cx.lock(){
            // acquire d3d11_cx exclusive
            for mut event in events {
                
                self.process_desktop_pre_event(&mut event, &mut event_handler);
                match &event {
                    Event::WindowSetHoverCursor(mc) => {
                        self.set_hover_mouse_cursor(mc.clone());
                    },
                    Event::WindowResizeLoop(wr) => {
                        for d3d11_window in &mut d3d11_windows {
                            if d3d11_window.window_id == wr.window_id {
                                if wr.was_started {
                                    d3d11_window.start_resize();
                                }
                                else {
                                    d3d11_window.stop_resize();
                                }
                            }
                        }
                    },
                    Event::WindowGeomChange(re) => { // do this here because mac
                        for d3d11_window in &mut d3d11_windows {
                            if d3d11_window.window_id == re.window_id {
                                d3d11_window.window_geom = re.new_geom.clone();
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
                    Event::WindowClosed(wc) => { // do this here because mac
                        // lets remove the window from the set
                        self.windows[wc.window_id].window_state = CxWindowState::Closed;
                        self.windows_free.push(wc.window_id);
                        // remove the d3d11/win32 window
                        
                        for index in 0..d3d11_windows.len() {
                            if d3d11_windows[index].window_id == wc.window_id {
                                d3d11_windows.remove(index);
                                if d3d11_windows.len() == 0 {
                                    win32_app.terminate_event_loop();
                                }
                                for d3d11_window in &mut d3d11_windows {
                                    d3d11_window.win32_window.update_ptrs();
                                }
                            }
                        }
                        self.call_event_handler(&mut event_handler, &mut event);
                    },
                    Event::Paint => {
                        self.repaint_id += 1;
                        let vsync = self.process_desktop_paint_callbacks(win32_app.time_now(), &mut event_handler);
                        
                        // construct or destruct windows
                        for (index, window) in self.windows.iter_mut().enumerate() {
                            
                            window.window_state = match &window.window_state {
                                CxWindowState::Create {inner_size, position, title} => {
                                    // lets create a platformwindow
                                    let d3d11_window = D3d11Window::new(index, &d3d11_cx, win32_app, *inner_size, *position, &title);
                                    window.window_geom = d3d11_window.window_geom.clone();
                                    d3d11_windows.push(d3d11_window);
                                    for d3d11_window in &mut d3d11_windows {
                                        d3d11_window.win32_window.update_ptrs();
                                    }
                                    CxWindowState::Created
                                },
                                CxWindowState::Close => {
                                    // ok we close the window
                                    // lets send it a WM_CLOSE event
                                    for d3d11_window in &mut d3d11_windows {if d3d11_window.window_id == index {
                                        d3d11_window.win32_window.close_window();
                                        if win32_app.event_loop_running == false{
                                            return false;
                                        }
                                        break;
                                    }}
                                    CxWindowState::Closed
                                },
                                CxWindowState::Created => CxWindowState::Created,
                                CxWindowState::Closed => CxWindowState::Closed
                            };
                            
                            if let Some(set_position) = window.window_set_position {
                                for d3d11_window in &mut d3d11_windows {if d3d11_window.window_id == index {
                                    d3d11_window.win32_window.set_position(set_position);
                                }}
                            }
                            
                            window.window_command = match &window.window_command {
                                CxWindowCmd::None => CxWindowCmd::None,
                                CxWindowCmd::Restore => {
                                    for d3d11_window in &mut d3d11_windows {if d3d11_window.window_id == index {
                                        d3d11_window.win32_window.restore();
                                    }}
                                    CxWindowCmd::None
                                },
                                CxWindowCmd::Maximize => {
                                    for d3d11_window in &mut d3d11_windows {if d3d11_window.window_id == index {
                                        d3d11_window.win32_window.maximize();
                                    }}
                                    CxWindowCmd::None
                                },
                                CxWindowCmd::Minimize => {
                                    for d3d11_window in &mut d3d11_windows {if d3d11_window.window_id == index {
                                        d3d11_window.win32_window.minimize();
                                    }}
                                    CxWindowCmd::None
                                },
                            };
                            
                            
                            window.window_set_position = None;
                            
                            if let Some(topmost) = window.window_topmost {
                                for d3d11_window in &mut d3d11_windows {if d3d11_window.window_id == index {
                                    d3d11_window.win32_window.set_topmost(topmost);
                                }}
                            }
                        }
                        
                        // set a cursor
                        if !self.down_mouse_cursor.is_none() {
                            win32_app.set_mouse_cursor(self.down_mouse_cursor.as_ref().unwrap().clone())
                        }
                        else if !self.hover_mouse_cursor.is_none() {
                            win32_app.set_mouse_cursor(self.hover_mouse_cursor.as_ref().unwrap().clone())
                        }
                        else {
                            win32_app.set_mouse_cursor(MouseCursor::Default)
                        }
                        
                        if let Some(set_ime_position) = self.platform.set_ime_position {
                            self.platform.set_ime_position = None;
                            for d3d11_window in &mut d3d11_windows {
                                d3d11_window.win32_window.set_ime_spot(set_ime_position);
                            }
                        }
                        
                        while self.platform.start_timer.len() > 0 {
                            let (timer_id, interval, repeats) = self.platform.start_timer.pop().unwrap();
                            win32_app.start_timer(timer_id, interval, repeats);
                        }
                        
                        while self.platform.stop_timer.len() > 0 {
                            let timer_id = self.platform.stop_timer.pop().unwrap();
                            win32_app.stop_timer(timer_id);
                        }
                        
                        // build a list of renderpasses to repaint
                        let mut windows_need_repaint = 0;
                        self.compute_passes_to_repaint(&mut passes_todo, &mut windows_need_repaint);
                        
                        if passes_todo.len() > 0 {
                            for pass_id in &passes_todo {
                                match self.passes[*pass_id].dep_of.clone() {
                                    CxPassDepOf::Window(window_id) => {
                                        // find the accompanying render window
                                        if let Some(d3d11_window) = d3d11_windows.iter_mut().find( | w | w.window_id == window_id) {
                                            windows_need_repaint -= 1;
                                            
                                            let dpi_factor = d3d11_window.window_geom.dpi_factor;
                                            self.passes[*pass_id].set_dpi_factor(dpi_factor);
                                            
                                            d3d11_window.resize_buffers(&d3d11_cx);
                                            
                                            self.draw_pass_to_window(
                                                *pass_id,
                                                vsync,
                                                dpi_factor,
                                                d3d11_window,
                                                &d3d11_cx,
                                            );
                                        }
                                        self.passes[*pass_id].paint_dirty = false;
                                    }
                                    CxPassDepOf::Pass(parent_pass_id) => {
                                        let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                                        self.passes[*pass_id].set_dpi_factor(dpi_factor);
                                        self.draw_pass_to_texture(
                                            *pass_id,
                                            dpi_factor,
                                            &d3d11_cx,
                                        );
                                        self.passes[*pass_id].paint_dirty = false;
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
                self.process_desktop_post_event(event);
            }
            if self.playing_anim_areas.len() == 0 && self.redraw_parent_areas.len() == 0 && self.redraw_child_areas.len() == 0 && self.frame_callbacks.len() == 0 {
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
}

struct D3d11RenderTarget {
    pub render_target_view: Option<ComPtr<d3d11::ID3D11RenderTargetView>>,
    //pub depth_stencil_view: Option<ComPtr<d3d11::ID3D11DepthStencilView>>,
    //pub depth_stencil_buffer: Option<ComPtr<d3d11::ID3D11Texture2D>>,
    pub raster_state: ComPtr<d3d11::ID3D11RasterizerState>,
    pub blend_state: ComPtr<d3d11::ID3D11BlendState>,
}

impl D3d11RenderTarget {
    fn new(d3d11_cx: &D3d11Cx) -> D3d11RenderTarget {
        let raster_state = d3d11_cx.create_raster_state().expect("Cannot create_raster_state");
        let blend_state = d3d11_cx.create_blend_state().expect("Cannot create_blend_state");
        return D3d11RenderTarget {
            raster_state: raster_state,
            blend_state: blend_state,
            render_target_view: None
        }
    }
    
    fn set_raster_and_blend_state(&self, d3d11_cx: &D3d11Cx) {
        unsafe {d3d11_cx.context.RSSetState(self.raster_state.as_raw() as *mut _)}
        let blend_factor = [0., 0., 0., 0.];
        unsafe {d3d11_cx.context.OMSetBlendState(self.blend_state.as_raw() as *mut _, &blend_factor, 0xffffffff)}
    }
    
    fn create_from_textures(&mut self, d3d11_cx: &D3d11Cx, swap_texture: &ComPtr<d3d11::ID3D11Texture2D>) {
        self.render_target_view = Some(d3d11_cx.create_render_target_view(swap_texture).expect("Cannot create_render_target_view"));
    }
    
    fn disconnect_render_targets(&mut self) {
        self.render_target_view = None;
    }
}

struct D3d11Window {
    pub window_id: usize,
    pub is_in_resize: bool,
    pub window_geom: WindowGeom,
    pub win32_window: Win32Window,
    pub render_target: D3d11RenderTarget,
    pub swap_texture: Option<ComPtr<d3d11::ID3D11Texture2D>>,
    // pub d2d1_hwnd_target: Option<ComPtr<d2d1::ID2D1HwndRenderTarget>>,
    // pub d2d1_bitmap: Option<ComPtr<d2d1::ID2D1Bitmap>>,
    pub alloc_size: Vec2,
    pub swap_chain: ComPtr<dxgi1_2::IDXGISwapChain1>,
}

impl D3d11Window {
    fn new(window_id: usize, d3d11_cx: &D3d11Cx, win32_app: &mut Win32App, inner_size: Vec2, position: Option<Vec2>, title: &str) -> D3d11Window {
        let mut win32_window = Win32Window::new(win32_app, window_id);
        
        win32_window.init(title, inner_size, position);
        let window_geom = win32_window.get_window_geom();
        
        let swap_chain = d3d11_cx.create_swap_chain_for_hwnd(&window_geom, &win32_window).expect("Cannot create_swap_chain_for_hwnd");
        
        let swap_texture = D3d11Cx::get_swap_texture(&swap_chain).expect("Cannot get swap texture");
        
        let mut render_target = D3d11RenderTarget::new(d3d11_cx);
        
        render_target.create_from_textures(d3d11_cx, &swap_texture);
        
        //let depth_stencil_buffer = d3d11_cx.create_depth_stencil_buffer(&window_geom).expect("Cannot create_depth_stencil_buffer");
        //let depth_stencil_view = d3d11_cx.create_depth_stencil_view(&depth_stencil_buffer).expect("Cannot create_depth_stencil_view");
        //let depth_stencil_state = d3d11_cx.create_depth_stencil_state().expect("Cannot create_depth_stencil_state");
        //unsafe {d3d11_cx.context.OMSetDepthStencilState(depth_stencil_state.as_raw() as *mut _, 1)}
        
        D3d11Window {
            is_in_resize: false,
            window_id: window_id,
            alloc_size: window_geom.inner_size,
            window_geom: window_geom,
            win32_window: win32_window,
            swap_texture: Some(swap_texture),
            render_target: render_target,
            //depth_stencil_buffer: Some(depth_stencil_buffer),
            //render_target_view: Some(render_target_view),
            //depth_stencil_view: Some(depth_stencil_view),
            //d2d1_hwnd_target: None,
            //d2d1_bitmap: None,
            swap_chain: swap_chain,
            //raster_state: raster_state,
            //blend_state: blend_state
        }
    }
    
    fn start_resize(&mut self) {
        self.is_in_resize = true;
    }
    
    // switch back to swapchain
    fn stop_resize(&mut self) {
        self.is_in_resize = false;
        self.alloc_size = Vec2::zero();
    }
    
    //fn alloc_buffers_from_texture(&mut self, d3d11_cx: &D3d11Cx) {
    //    let swap_texture = self.swap_texture.as_ref().unwrap();
    //    let render_target_view = d3d11_cx.create_render_target_view(&swap_texture).expect("Cannot create_render_target_view");
    //let depth_stencil_buffer = d3d11_cx.create_depth_stencil_buffer(&self.window_geom).expect("Cannot create_depth_stencil_buffer");
    //let depth_stencil_view = d3d11_cx.create_depth_stencil_view(&depth_stencil_buffer).expect("Cannot create_depth_stencil_view");
    //    self.alloc_size = self.window_geom.inner_size;
    //    self.render_target_view = Some(render_target_view);
    //    self.depth_stencil_view = Some(depth_stencil_view);
    //    self.depth_stencil_buffer = Some(depth_stencil_buffer);
    // }
    
    fn resize_buffers(&mut self, d3d11_cx: &D3d11Cx) {
        if self.alloc_size == self.window_geom.inner_size {
            return
        }
        
        self.render_target.disconnect_render_targets();
        self.swap_texture = None;
        d3d11_cx.disconnect_rendertargets();
        
        d3d11_cx.resize_swap_chain(&self.window_geom, &self.swap_chain);
        
        let swap_texture = D3d11Cx::get_swap_texture(&self.swap_chain).expect("Cannot get swap texture");
        
        self.render_target.create_from_textures(d3d11_cx, &swap_texture);
        self.swap_texture = Some(swap_texture);
    }
    
    fn present(&mut self, vsync: bool) {
        unsafe {self.swap_chain.Present(if vsync {1}else {0}, 0)};
    }
    
}

#[derive(Default)]
pub struct CxPlatform {
    pub uni_cx: D3d11Buffer,
    pub post_id: u64,
    pub set_ime_position: Option<Vec2>,
    pub start_timer: Vec<(u64, f64, bool)>,
    pub stop_timer: Vec<(u64)>,
    pub text_clipboard_response: Option<String>,
    pub desktop: CxDesktop,
    pub d3d11_cx: Option<*const D3d11Cx>
}

#[derive(Clone)]
pub struct D3d11Cx {
    pub device: ComPtr<d3d11::ID3D11Device>,
    pub context: ComPtr<d3d11::ID3D11DeviceContext>,
    pub factory: ComPtr<dxgi1_2::IDXGIFactory2>,
    //    pub d2d1_factory: ComPtr<d2d1::ID2D1Factory>
}

impl D3d11Cx {
    
    fn new() -> D3d11Cx {
        let factory = D3d11Cx::create_dxgi_factory1(&dxgi1_2::IDXGIFactory2::uuidof()).expect("cannot create_dxgi_factory1");
        let adapter = D3d11Cx::enum_adapters(&factory).expect("cannot enum_adapters");
        let (device, context) = D3d11Cx::create_d3d11_device(&adapter).expect("cannot create_d3d11_device");
        // let d2d1_factory = D3d11Cx::create_d2d1_factory().expect("cannot create_d2d1_factory");
        D3d11Cx {
            device: device,
            context: context,
            factory: factory,
            //    d2d1_factory: d2d1_factory
        }
    }
    
    fn disconnect_rendertargets(&self) {
        unsafe {self.context.OMSetRenderTargets(0, ptr::null(), ptr::null_mut())}
    }
    
    fn set_viewport(&self, width: f32, height: f32) {
        let viewport = d3d11::D3D11_VIEWPORT {
            Width: width,
            Height: height,
            MinDepth: 0.,
            MaxDepth: 1.,
            TopLeftX: 0.0,
            TopLeftY: 0.0
        };
        unsafe {self.context.RSSetViewports(1, &viewport)}
    }
    
    fn set_rendertargets(&self, d3d11_target: &D3d11RenderTarget) {
        unsafe {self.context.OMSetRenderTargets(
            1,
            [d3d11_target.render_target_view.as_ref().unwrap().as_raw() as *mut _].as_ptr(),
            ptr::null_mut() //,//d3d11_window.depth_stencil_view.as_ref().unwrap().as_raw() as *mut _
        )}
    }
    
    fn clear_render_target_view(&self, d3d11_target: &D3d11RenderTarget, color: Color) {
        let color = [color.r, color.g, color.b, color.a];
        unsafe {self.context.ClearRenderTargetView(d3d11_target.render_target_view.as_ref().unwrap().as_raw() as *mut _, &color)}
    }
    
    //fn clear_depth_stencil_view(&self, d3d11_window: &D3d11Window) {
    //    unsafe {self.context.ClearDepthStencilView(d3d11_window.depth_stencil_view.as_ref().unwrap().as_raw() as *mut _, d3d11::D3D11_CLEAR_DEPTH, 1.0, 0)}
    //}
    
    fn set_input_layout(&self, input_layout: &ComPtr<d3d11::ID3D11InputLayout>) {
        unsafe {self.context.IASetInputLayout(input_layout.as_raw() as *mut _)}
    }
    
    fn set_index_buffer(&self, index_buffer: &D3d11Buffer) {
        if let Some(buf) = &index_buffer.buffer {
            unsafe {self.context.IASetIndexBuffer(buf.as_raw() as *mut _, dxgiformat::DXGI_FORMAT_R32_UINT, 0)}
        }
    }
    
    fn set_shaders(&self, vertex_shader: &ComPtr<d3d11::ID3D11VertexShader>, pixel_shader: &ComPtr<d3d11::ID3D11PixelShader>) {
        unsafe {self.context.VSSetShader(vertex_shader.as_raw() as *mut _, ptr::null(), 0)}
        unsafe {self.context.PSSetShader(pixel_shader.as_raw() as *mut _, ptr::null(), 0)}
    }
    
    fn set_shader_resource(&self, index: usize, texture: &Option<ComPtr<d3d11::ID3D11ShaderResourceView>>) {
        if let Some(texture) = texture {
            let raw = [texture.as_raw() as *const std::ffi::c_void];
            unsafe {self.context.PSSetShaderResources(index as u32, 1, raw.as_ptr() as *const *mut _)}
            unsafe {self.context.VSSetShaderResources(index as u32, 1, raw.as_ptr() as *const *mut _)}
        }
    }
    
    //fn set_raster_state(&self, d3d11_window: &D3d11Window) {
    //    unsafe {self.context.RSSetState(d3d11_window.raster_state.as_raw() as *mut _)};
    // }
    
    fn set_primitive_topology(&self) {
        unsafe {self.context.IASetPrimitiveTopology(d3dcommon::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST)}
    }
    
    fn set_vertex_buffers(&self, geom_buf: &D3d11Buffer, geom_slots: usize, inst_buf: &D3d11Buffer, inst_slots: usize) {
        let geom_buf = geom_buf.buffer.as_ref().unwrap();
        let inst_buf = inst_buf.buffer.as_ref().unwrap();
        
        let strides = [(geom_slots * 4) as u32, (inst_slots * 4) as u32];
        let offsets = [0u32, 0u32];
        let buffers = [geom_buf.as_raw() as *const std::ffi::c_void, inst_buf.as_raw() as *const std::ffi::c_void];
        unsafe {self.context.IASetVertexBuffers(
            0,
            2,
            buffers.as_ptr() as *const *mut _,
            strides.as_ptr() as *const _,
            offsets.as_ptr() as *const _
        )};
    }
    
    fn set_constant_buffers(&self, cx_buf: &D3d11Buffer, dl_buf: &D3d11Buffer, dr_buf: &D3d11Buffer) {
        let cx_buf = cx_buf.buffer.as_ref().unwrap();
        let dl_buf = dl_buf.buffer.as_ref().unwrap();
        let dr_buf = dr_buf.buffer.as_ref().unwrap();
        let buffers = [
            cx_buf.as_raw() as *const std::ffi::c_void,
            dl_buf.as_raw() as *const std::ffi::c_void,
            dr_buf.as_raw() as *const std::ffi::c_void
        ];
        unsafe {self.context.VSSetConstantBuffers(
            0,
            3,
            buffers.as_ptr() as *const *mut _,
        )};
        unsafe {self.context.PSSetConstantBuffers(
            0,
            3,
            buffers.as_ptr() as *const *mut _,
        )};
    }
    
    fn draw_indexed_instanced(&self, num_vertices: usize, num_instances: usize) {
        
        
        unsafe {self.context.DrawIndexedInstanced(
            num_vertices as u32,
            num_instances as u32,
            0,
            0,
            0
        )};
    }
    
    pub fn compile_shader(&self, stage: &str, entry: &[u8], shader: &[u8])
        -> Result<ComPtr<d3dcommon::ID3DBlob>, SlErr> {
        
        let mut blob = ptr::null_mut();
        let mut error = ptr::null_mut();
        let entry = ffi::CString::new(entry).unwrap();
        let stage = format!("{}_5_0\0", stage);
        let hr = unsafe {d3dcompiler::D3DCompile(
            shader.as_ptr() as *const _,
            shader.len(),
            ptr::null(),
            ptr::null(),
            ptr::null_mut(),
            entry.as_ptr() as *const _,
            stage.as_ptr() as *const i8,
            1,
            0,
            &mut blob as *mut *mut _,
            &mut error as *mut *mut _
        )};
        if !winerror::SUCCEEDED(hr) {
            let error = unsafe {ComPtr::<d3dcommon::ID3DBlob>::from_raw(error)};
            let message = unsafe {
                let pointer = error.GetBufferPointer();
                let size = error.GetBufferSize();
                let slice = std::slice::from_raw_parts(pointer as *const u8, size as usize);
                String::from_utf8_lossy(slice).into_owned()
            };
            
            Err(SlErr {msg: message})
        } else {
            Ok(unsafe {ComPtr::<d3dcommon::ID3DBlob>::from_raw(blob)})
        }
    }
    
    pub fn create_input_layout(&self, vs: &ComPtr<d3dcommon::ID3DBlob>, layout_desc: &Vec<d3d11::D3D11_INPUT_ELEMENT_DESC>)
        -> Result<ComPtr<d3d11::ID3D11InputLayout>, SlErr> {
        let mut input_layout = ptr::null_mut();
        let hr = unsafe {self.device.CreateInputLayout(
            layout_desc.as_ptr(),
            layout_desc.len() as u32,
            vs.GetBufferPointer(),
            vs.GetBufferSize(),
            &mut input_layout as *mut *mut _
        )};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(input_layout as *mut _)})
        }
        else {
            Err(SlErr {msg: format!("create_input_layout failed {}", hr)})
        }
    }
    
    pub fn create_pixel_shader(&self, ps: &ComPtr<d3dcommon::ID3DBlob>)
        -> Result<ComPtr<d3d11::ID3D11PixelShader>, SlErr> {
        let mut pixel_shader = ptr::null_mut();
        let hr = unsafe {self.device.CreatePixelShader(
            ps.GetBufferPointer(),
            ps.GetBufferSize(),
            ptr::null_mut(),
            &mut pixel_shader as *mut *mut _
        )};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(pixel_shader as *mut _)})
        }
        else {
            Err(SlErr {msg: format!("create_pixel_shader failed {}", hr)})
        }
    }
    
    
    pub fn create_vertex_shader(&self, vs: &ComPtr<d3dcommon::ID3DBlob>)
        -> Result<ComPtr<d3d11::ID3D11VertexShader>, SlErr> {
        let mut vertex_shader = ptr::null_mut();
        let hr = unsafe {self.device.CreateVertexShader(
            vs.GetBufferPointer(),
            vs.GetBufferSize(),
            ptr::null_mut(),
            &mut vertex_shader as *mut *mut _
        )};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(vertex_shader as *mut _)})
        }
        else {
            Err(SlErr {msg: format!("create_vertex_shader failed {}", hr)})
        }
    }
    
    
    fn create_raster_state(&self)
        -> Result<ComPtr<d3d11::ID3D11RasterizerState>, winerror::HRESULT> {
        let mut raster_state = ptr::null_mut();
        let raster_desc = d3d11::D3D11_RASTERIZER_DESC {
            AntialiasedLineEnable: FALSE,
            CullMode: d3d11::D3D11_CULL_NONE,
            DepthBias: 0,
            DepthBiasClamp: 0.0,
            DepthClipEnable: TRUE,
            FillMode: d3d11::D3D11_FILL_SOLID,
            FrontCounterClockwise: FALSE,
            MultisampleEnable: FALSE,
            ScissorEnable: FALSE,
            SlopeScaledDepthBias: 0.0,
        };
        let hr = unsafe {self.device.CreateRasterizerState(&raster_desc, &mut raster_state as *mut *mut _)};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(raster_state as *mut _)})
        }
        else {
            Err(hr)
        }
    }
    
    fn create_blend_state(&self)
        -> Result<ComPtr<d3d11::ID3D11BlendState>, winerror::HRESULT> {
        let mut blend_state = ptr::null_mut();
        let mut blend_desc: d3d11::D3D11_BLEND_DESC = unsafe {mem::zeroed()};
        blend_desc.AlphaToCoverageEnable = FALSE;
        blend_desc.RenderTarget[0] = d3d11::D3D11_RENDER_TARGET_BLEND_DESC {
            BlendEnable: TRUE,
            SrcBlend: d3d11::D3D11_BLEND_ONE,
            SrcBlendAlpha: d3d11::D3D11_BLEND_ONE,
            DestBlend: d3d11::D3D11_BLEND_INV_SRC_ALPHA,
            DestBlendAlpha: d3d11::D3D11_BLEND_INV_SRC_ALPHA,
            BlendOp: d3d11::D3D11_BLEND_OP_ADD,
            BlendOpAlpha: d3d11::D3D11_BLEND_OP_ADD,
            RenderTargetWriteMask: d3d11::D3D11_COLOR_WRITE_ENABLE_ALL as u8,
        };
        let hr = unsafe {self.device.CreateBlendState(&blend_desc, &mut blend_state as *mut *mut _)};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(blend_state as *mut _)})
        }
        else {
            Err(hr)
        }
    }
    
    fn create_depth_stencil_view(&self, buffer: &ComPtr<d3d11::ID3D11Texture2D>)
        -> Result<ComPtr<d3d11::ID3D11DepthStencilView>, winerror::HRESULT> {
        let mut depth_stencil_view = ptr::null_mut();
        let mut dsv_desc: d3d11::D3D11_DEPTH_STENCIL_VIEW_DESC = unsafe {mem::zeroed()};
        dsv_desc.Format = dxgiformat::DXGI_FORMAT_D24_UNORM_S8_UINT;
        dsv_desc.ViewDimension = d3d11::D3D11_DSV_DIMENSION_TEXTURE2D;
        *unsafe {dsv_desc.u.Texture2D_mut()} = d3d11::D3D11_TEX2D_DSV {
            MipSlice: 0,
        };
        let hr = unsafe {self.device.CreateDepthStencilView(buffer.as_raw() as *mut _, &dsv_desc, &mut depth_stencil_view as *mut *mut _)};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(depth_stencil_view as *mut _)})
        }
        else {
            Err(hr)
        }
    }
    
    fn create_depth_stencil_state(&self)
        -> Result<ComPtr<d3d11::ID3D11DepthStencilState>, winerror::HRESULT> {
        let mut depth_stencil_state = ptr::null_mut();
        let ds_desc = d3d11::D3D11_DEPTH_STENCIL_DESC {
            DepthEnable: FALSE,
            DepthWriteMask: d3d11::D3D11_DEPTH_WRITE_MASK_ALL,
            DepthFunc: d3d11::D3D11_COMPARISON_ALWAYS,
            StencilEnable: FALSE,
            StencilReadMask: 0xff,
            StencilWriteMask: 0xff,
            FrontFace: d3d11::D3D11_DEPTH_STENCILOP_DESC {
                StencilFailOp: d3d11::D3D11_STENCIL_OP_REPLACE,
                StencilDepthFailOp: d3d11::D3D11_STENCIL_OP_REPLACE,
                StencilPassOp: d3d11::D3D11_STENCIL_OP_REPLACE,
                StencilFunc: d3d11::D3D11_COMPARISON_ALWAYS,
            },
            BackFace: d3d11::D3D11_DEPTH_STENCILOP_DESC {
                StencilFailOp: d3d11::D3D11_STENCIL_OP_REPLACE,
                StencilDepthFailOp: d3d11::D3D11_STENCIL_OP_REPLACE,
                StencilPassOp: d3d11::D3D11_STENCIL_OP_REPLACE,
                StencilFunc: d3d11::D3D11_COMPARISON_ALWAYS,
            },
        };
        
        let hr = unsafe {self.device.CreateDepthStencilState(&ds_desc, &mut depth_stencil_state as *mut *mut _)};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(depth_stencil_state as *mut _)})
        }
        else {
            Err(hr)
        }
    }
    
    fn create_render_target_view(&self, texture: &ComPtr<d3d11::ID3D11Texture2D>)
        -> Result<ComPtr<d3d11::ID3D11RenderTargetView>, winerror::HRESULT> {
        //let texture = D3d11Cx::get_swap_texture(swap_chain) ?;
        let mut render_target_view = ptr::null_mut();
        let hr = unsafe {self.device.CreateRenderTargetView(
            texture.as_raw() as *mut _,
            ptr::null(),
            &mut render_target_view as *mut *mut _
        )};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(render_target_view as *mut _)})
        }
        else {
            Err(hr)
        }
    }
    
    fn get_swap_texture(swap_chain: &ComPtr<dxgi1_2::IDXGISwapChain1>)
        -> Result<ComPtr<d3d11::ID3D11Texture2D>, winerror::HRESULT> {
        let mut texture = ptr::null_mut();
        let hr = unsafe {swap_chain.GetBuffer(
            0,
            &d3d11::ID3D11Texture2D::uuidof(),
            &mut texture as *mut *mut _
        )};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(texture as *mut _)})
        }
        else {
            Err(hr)
        }
    }
    
    fn create_depth_stencil_buffer(&self, wg: &WindowGeom)
        -> Result<ComPtr<d3d11::ID3D11Texture2D>, winerror::HRESULT> {
        let mut depth_stencil_buffer = ptr::null_mut();
        let texture2d_desc = d3d11::D3D11_TEXTURE2D_DESC {
            Width: (wg.inner_size.x * wg.dpi_factor) as u32,
            Height: (wg.inner_size.y * wg.dpi_factor) as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: dxgiformat::DXGI_FORMAT_D24_UNORM_S8_UINT,
            SampleDesc: dxgitype::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0
            },
            Usage: d3d11::D3D11_USAGE_DEFAULT,
            BindFlags: d3d11::D3D11_BIND_DEPTH_STENCIL,
            CPUAccessFlags: 0,
            MiscFlags: 0
        };
        let hr = unsafe {self.device.CreateTexture2D(&texture2d_desc, ptr::null_mut(), &mut depth_stencil_buffer as *mut *mut _)};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(depth_stencil_buffer as *mut _)})
        }
        else {
            Err(hr)
        }
    }
    
    fn create_swap_chain_for_hwnd(
        &self,
        wg: &WindowGeom,
        win32_window: &Win32Window,
    )
        -> Result<ComPtr<dxgi1_2::IDXGISwapChain1>, winerror::HRESULT> {
        let mut swap_chain1 = ptr::null_mut();
        let sc_desc = dxgi1_2::DXGI_SWAP_CHAIN_DESC1 {
            AlphaMode: dxgi1_2::DXGI_ALPHA_MODE_IGNORE,
            BufferCount: 2,
            Width: (wg.inner_size.x * wg.dpi_factor) as u32,
            Height: (wg.inner_size.y * wg.dpi_factor) as u32,
            Format: dxgiformat::DXGI_FORMAT_B8G8R8A8_UNORM,
            Flags: 0,
            BufferUsage: dxgitype::DXGI_USAGE_RENDER_TARGET_OUTPUT,
            SampleDesc: dxgitype::DXGI_SAMPLE_DESC {Count: 1, Quality: 0,},
            Scaling: dxgi1_2::DXGI_SCALING_NONE,
            Stereo: FALSE,
            SwapEffect: dxgi::DXGI_SWAP_EFFECT_FLIP_DISCARD,
        };
        
        let hr = unsafe {self.factory.CreateSwapChainForHwnd(
            self.device.as_raw() as *mut _,
            win32_window.hwnd.unwrap(),
            &sc_desc,
            ptr::null(),
            ptr::null_mut(),
            &mut swap_chain1 as *mut *mut _
        )};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(swap_chain1 as *mut _)})
        }
        else {
            Err(hr)
        }
    }
    
    fn resize_swap_chain(
        &self,
        wg: &WindowGeom,
        swap_chain: &ComPtr<dxgi1_2::IDXGISwapChain1>,
    ) {
        unsafe {
            let hr = swap_chain.ResizeBuffers(
                2,
                (wg.inner_size.x * wg.dpi_factor) as u32,
                (wg.inner_size.y * wg.dpi_factor) as u32,
                dxgiformat::DXGI_FORMAT_B8G8R8A8_UNORM,
                0
            );
            if !winerror::SUCCEEDED(hr) {
                panic!("Could not resize swapchain");
            }
        }
    }
    
    fn create_d3d11_device(adapter: &ComPtr<dxgi::IDXGIAdapter>)
        -> Result<(ComPtr<d3d11::ID3D11Device>, ComPtr<d3d11::ID3D11DeviceContext>), winerror::HRESULT> {
        let mut device = ptr::null_mut();
        let mut device_context = ptr::null_mut();
        let hr = unsafe {d3d11::D3D11CreateDevice(
            adapter.as_raw() as *mut _,
            d3dcommon::D3D_DRIVER_TYPE_UNKNOWN,
            ptr::null_mut(),
            d3d11::D3D11_CREATE_DEVICE_VIDEO_SUPPORT, //d3dcommonLLD3D11_CREATE_DEVICE_DEBUG,
            [d3dcommon::D3D_FEATURE_LEVEL_11_0].as_ptr(),
            1,
            d3d11::D3D11_SDK_VERSION,
            &mut device as *mut *mut _,
            ptr::null_mut(),
            &mut device_context as *mut *mut _,
        )};
        if winerror::SUCCEEDED(hr) {
            Ok((unsafe {ComPtr::from_raw(device as *mut _)}, unsafe {ComPtr::from_raw(device_context as *mut _)}))
        }
        else {
            Err(hr)
        }
    }
    
    fn create_dxgi_factory1(guid: &GUID)
        -> Result<ComPtr<dxgi1_2::IDXGIFactory2>, winerror::HRESULT> {
        let mut factory = ptr::null_mut();
        let hr = unsafe {dxgi::CreateDXGIFactory1(
            guid,
            &mut factory as *mut *mut _
        )};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(factory as *mut _)})
        }
        else {
            Err(hr)
        }
    }
    
    
    fn enum_adapters(factory: &ComPtr<dxgi1_2::IDXGIFactory2>)
        -> Result<ComPtr<dxgi::IDXGIAdapter>, winerror::HRESULT> {
        let mut adapter = ptr::null_mut();
        let hr = unsafe {factory.EnumAdapters(
            0,
            &mut adapter as *mut *mut _
        )};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(adapter as *mut _)})
        }
        else {
            Err(hr)
        }
    }
    
    pub fn init_mapped_textures(&self, mapped: &mut CxPlatformTextureMapped, texture_format: &TextureFormat, width: usize, height: usize) {
        let slots_per_pixel;
        let format;
        match texture_format {
            TextureFormat::MappedRGf32 => {
                slots_per_pixel = 2;
                format = dxgiformat::DXGI_FORMAT_R32G32_FLOAT;
            }
            TextureFormat::MappedRf32 => {
                slots_per_pixel = 1;
                format = dxgiformat::DXGI_FORMAT_R32_FLOAT;
            }
            TextureFormat::MappedBGRA => {
                slots_per_pixel = 1;
                format = dxgiformat::DXGI_FORMAT_R8G8B8A8_UNORM;
            }
            TextureFormat::MappedBGRAf32 => {
                slots_per_pixel = 4;
                format = dxgiformat::DXGI_FORMAT_R32G32B32A32_FLOAT;
            },
            _ => {
                panic!("Wrong format for init_mapped_texures");
            }
        }
        let texture_desc = d3d11::D3D11_TEXTURE2D_DESC {
            Width: width as u32,
            Height: height as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: dxgitype::DXGI_SAMPLE_DESC {Count: 1, Quality: 0},
            Usage: d3d11::D3D11_USAGE_DYNAMIC,
            BindFlags: d3d11::D3D11_BIND_SHADER_RESOURCE,
            CPUAccessFlags: d3d11::D3D11_CPU_ACCESS_WRITE,
            MiscFlags: 0,
        };
        for i in 0..MAPPED_TEXTURE_BUFFER_COUNT {
            let mut texture = ptr::null_mut();
            let hr = unsafe {self.device.CreateTexture2D(&texture_desc, ptr::null(), &mut texture as *mut *mut _)};
            if winerror::SUCCEEDED(hr) {
                let mut shader_resource = ptr::null_mut();
                unsafe {self.device.CreateShaderResourceView(texture as *mut _, ptr::null(), &mut shader_resource as *mut *mut _)};
                mapped.textures[i].slots_per_pixel = slots_per_pixel;
                mapped.textures[i].width = width;
                mapped.textures[i].height = height;
                mapped.textures[i].texture = Some(unsafe {ComPtr::from_raw(texture as *mut _)});
                let mut shader_resource = ptr::null_mut();
                unsafe {self.device.CreateShaderResourceView(texture as *mut _, ptr::null(), &mut shader_resource as *mut *mut _)};
                
                mapped.textures[i].shader_resource = Some(unsafe {ComPtr::from_raw(shader_resource as *mut _)});
                let mut d3d11_resource = ptr::null_mut();
                unsafe {mapped.textures[i].texture.as_ref().unwrap().QueryInterface(
                    &d3d11::ID3D11Resource::uuidof(),
                    &mut d3d11_resource as *mut *mut _
                )};
                mapped.textures[i].d3d11_resource = Some(unsafe {ComPtr::from_raw(d3d11_resource as *mut _)});
                // map them by default
                self.map_texture(&mut mapped.textures[i]);
            }
            else {
                panic!("init_mapped_textures failed");
            }
        }
    }
    
    pub fn update_render_target(&self, cxtexture: &mut CxTexture, dpi_factor: f32, size: Vec2) {
        
        let width = if let Some(width) = cxtexture.desc.width {width as usize} else {(size.x * dpi_factor) as usize};
        let height = if let Some(height) = cxtexture.desc.height {height as usize} else {(size.y * dpi_factor) as usize};
        
        if cxtexture.platform.single.width == width
            && cxtexture.platform.single.height == height {
            return
        }
        
        let format;
        match cxtexture.desc.format {
            TextureFormat::Default | TextureFormat::RenderBGRA => {
                format = dxgiformat::DXGI_FORMAT_R8G8B8A8_UNORM;
            }
            TextureFormat::RenderBGRAf16 => {
                format = dxgiformat::DXGI_FORMAT_R32G32B32A32_FLOAT;
            }
            TextureFormat::RenderBGRAf32 => {
                format = dxgiformat::DXGI_FORMAT_R32G32B32A32_FLOAT;
            },
            _ => {
                panic!("Wrong format for update_render_target");
            }
        }
        let texture_desc = d3d11::D3D11_TEXTURE2D_DESC {
            Width: width as u32,
            Height: height as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: dxgitype::DXGI_SAMPLE_DESC {Count: 1, Quality: 0},
            Usage: d3d11::D3D11_USAGE_DEFAULT,
            BindFlags: d3d11::D3D11_BIND_RENDER_TARGET | d3d11::D3D11_BIND_SHADER_RESOURCE,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        
        let mut texture = ptr::null_mut();
        let hr = unsafe {self.device.CreateTexture2D(&texture_desc, ptr::null(), &mut texture as *mut *mut _)};
        
        if winerror::SUCCEEDED(hr) {
            let mut shader_resource = ptr::null_mut();
            unsafe {self.device.CreateShaderResourceView(texture as *mut _, ptr::null(), &mut shader_resource as *mut *mut _)};
            cxtexture.platform.single.width = width;
            cxtexture.platform.single.height = height;
            cxtexture.platform.single.texture = Some(unsafe {ComPtr::from_raw(texture as *mut _)});
            let mut shader_resource = ptr::null_mut();
            unsafe {self.device.CreateShaderResourceView(texture as *mut _, ptr::null(), &mut shader_resource as *mut *mut _)};
            cxtexture.platform.single.shader_resource = Some(unsafe {ComPtr::from_raw(shader_resource as *mut _)});
            // lets create the render target too
            let mut render_target = D3d11RenderTarget::new(self);
            render_target.create_from_textures(self, cxtexture.platform.single.texture.as_ref().unwrap());
            cxtexture.platform.render = Some(render_target);
        }
        else {
            panic!("init_render_texture failed");
        }
    }
    
    fn flip_mapped_textures(&self, mapped: &mut CxPlatformTextureMapped, repaint_id: u64) -> Option<usize> {
        if mapped.repaint_id == repaint_id {
            return mapped.repaint_read
        }
        mapped.repaint_id = repaint_id;
        for i in 0..MAPPED_TEXTURE_BUFFER_COUNT {
            if mapped.write > mapped.read && i == mapped.read % MAPPED_TEXTURE_BUFFER_COUNT {
                if mapped.textures[i].locked {
                    panic!("Texture locked whilst unmapping, should never happen!")
                }
                self.unmap_texture(&mut mapped.textures[i]);
            }
            else if mapped.read == 0 || i != (mapped.read - 1) % MAPPED_TEXTURE_BUFFER_COUNT { // keep a safety for not stalling gpu
                self.map_texture(&mut mapped.textures[i]);
            }
        }
        if mapped.read != mapped.write && mapped.textures[mapped.read % MAPPED_TEXTURE_BUFFER_COUNT].map_ptr.is_none() {
            let read = mapped.read;
            if mapped.read != mapped.write && mapped.write > mapped.read {
                if mapped.write > mapped.read + 1 { // space ahead
                    mapped.read += 1;
                }
            }
            mapped.repaint_read = Some(read % MAPPED_TEXTURE_BUFFER_COUNT);
            return mapped.repaint_read;
        }
        mapped.repaint_read = None;
        return None
    }
    
    fn map_texture(&self, texture: &mut CxPlatformTextureResource) {
        if !texture.map_ptr.is_none() {
            return
        }
        unsafe {
            let mut mapped_resource = std::mem::zeroed();
            let d3d11_resource = texture.d3d11_resource.as_ref().unwrap();
            let hr = self.context.Map(d3d11_resource.as_raw(), 0, d3d11::D3D11_MAP_WRITE_DISCARD, 0, &mut mapped_resource);
            if winerror::SUCCEEDED(hr) {
                texture.map_ptr = Some(mapped_resource.pData);
            }
        }
    }
    
    fn unmap_texture(&self, texture: &mut CxPlatformTextureResource) {
        if texture.map_ptr.is_none() {
            return
        }
        let d3d11_resource = texture.d3d11_resource.as_ref().unwrap();
        unsafe {
            self.context.Unmap(d3d11_resource.as_raw(), 0);
        }
        texture.map_ptr = None;
    }
    
    pub fn update_platform_texture_image_bgra(&self, res: &mut CxPlatformTextureResource, width: usize, height: usize, image_u32: &Vec<u32>) {
        
        if image_u32.len() != width * height {
            println!("update_platform_texture_image_bgra with wrong buffer_u32 size!");
            return;
        }
        
        let sub_data = d3d11::D3D11_SUBRESOURCE_DATA {
            pSysMem: image_u32.as_ptr() as *const _,
            SysMemPitch: (width * 4) as u32,
            SysMemSlicePitch: 0
        };
        
        let texture_desc = d3d11::D3D11_TEXTURE2D_DESC {
            Width: width as u32,
            Height: height as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: dxgiformat::DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: dxgitype::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0
            },
            Usage: d3d11::D3D11_USAGE_DEFAULT,
            BindFlags: d3d11::D3D11_BIND_SHADER_RESOURCE,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut texture = ptr::null_mut();
        let hr = unsafe {self.device.CreateTexture2D(&texture_desc, &sub_data, &mut texture as *mut *mut _)};
        if winerror::SUCCEEDED(hr) {
            let mut shader_resource = ptr::null_mut();
            unsafe {self.device.CreateShaderResourceView(
                texture as *mut _,
                ptr::null(),
                &mut shader_resource as *mut *mut _
            )};
            res.width = width;
            res.height = height;
            res.texture = Some(unsafe {ComPtr::from_raw(texture as *mut _)});
            res.shader_resource = Some(unsafe {ComPtr::from_raw(shader_resource as *mut _)});
        }
        else {
            panic!("update_platform_texture_image_bgra failed");
        }
    }
    
    pub fn update_platform_texture_image_rf32(&self, res: &mut CxPlatformTextureResource, width: usize, height: usize, image_f32: &Vec<f32>) {
        if image_f32.len() != width * height {
            println!("update_platform_texture_image_rf32 with wrong buffer_u32 size!");
            return;
        }
        
        let mut texture = ptr::null_mut();
        let sub_data = d3d11::D3D11_SUBRESOURCE_DATA {
            pSysMem: image_f32.as_ptr() as *const _,
            SysMemPitch: (width * 4) as u32,
            SysMemSlicePitch: 0
        };
        
        let texture_desc = d3d11::D3D11_TEXTURE2D_DESC {
            Width: width as u32,
            Height: height as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: dxgiformat::DXGI_FORMAT_R32_FLOAT,
            SampleDesc: dxgitype::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0
            },
            Usage: d3d11::D3D11_USAGE_DEFAULT,
            BindFlags: d3d11::D3D11_BIND_SHADER_RESOURCE,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        
        let hr = unsafe {self.device.CreateTexture2D(&texture_desc, &sub_data, &mut texture as *mut *mut _)};
        if winerror::SUCCEEDED(hr) {
            let mut shader_resource = ptr::null_mut();
            unsafe {self.device.CreateShaderResourceView(
                texture as *mut _,
                ptr::null(),
                &mut shader_resource as *mut *mut _
            )};
            res.width = width;
            res.height = height;
            res.texture = Some(unsafe {ComPtr::from_raw(texture as *mut _)});
            res.shader_resource = Some(unsafe {ComPtr::from_raw(shader_resource as *mut _)});
        }
        else {
            panic!("update_platform_texture_image_rf32 failed");
        }
    }
    
    
    pub fn update_platform_texture_image_rgf32(&self, res: &mut CxPlatformTextureResource, width: usize, height: usize, image_f32: &Vec<f32>) {
        if image_f32.len() != width * height * 2 {
            println!("update_platform_texture_image_rgf32 with wrong buffer_f32 size {} {}!", width * height * 2, image_f32.len());
            return;
        }
        
        let mut texture = ptr::null_mut();
        let sub_data = d3d11::D3D11_SUBRESOURCE_DATA {
            pSysMem: image_f32.as_ptr() as *const _,
            SysMemPitch: (width * 8) as u32,
            SysMemSlicePitch: 0
        };
        
        let texture_desc = d3d11::D3D11_TEXTURE2D_DESC {
            Width: width as u32,
            Height: height as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: dxgiformat::DXGI_FORMAT_R32G32_FLOAT,
            SampleDesc: dxgitype::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0
            },
            Usage: d3d11::D3D11_USAGE_DEFAULT,
            BindFlags: d3d11::D3D11_BIND_SHADER_RESOURCE,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        
        let hr = unsafe {self.device.CreateTexture2D(&texture_desc, &sub_data, &mut texture as *mut *mut _)};
        if winerror::SUCCEEDED(hr) {
            let mut shader_resource = ptr::null_mut();
            unsafe {self.device.CreateShaderResourceView(
                texture as *mut _,
                ptr::null(),
                &mut shader_resource as *mut *mut _
            )};
            res.texture = Some(unsafe {ComPtr::from_raw(texture as *mut _)});
            res.width = width;
            res.height = height;
            res.shader_resource = Some(unsafe {ComPtr::from_raw(shader_resource as *mut _)});
        }
        else {
            panic!("update_platform_texture_image_rgf32 failed");
        }
    }
}

#[derive(Clone, Default)]
pub struct CxPlatformView {
    pub uni_dl: D3d11Buffer
}

#[derive(Default, Clone, Debug)]
pub struct PlatformDrawCall {
    pub uni_dr: D3d11Buffer,
    pub inst_vbuf: D3d11Buffer
}

#[derive(Default, Clone, Debug)]
pub struct D3d11Buffer {
    pub last_size: usize,
    pub buffer: Option<ComPtr<d3d11::ID3D11Buffer>>
}

impl D3d11Buffer {
    pub fn update_with_data(&mut self, d3d11_cx: &D3d11Cx, bind_flags: u32, len_slots: usize, data: *const std::ffi::c_void) {
        let mut buffer = ptr::null_mut();
        
        let buffer_desc = d3d11::D3D11_BUFFER_DESC {
            Usage: d3d11::D3D11_USAGE_DEFAULT,
            ByteWidth: (len_slots * 4) as u32,
            BindFlags: bind_flags,
            CPUAccessFlags: 0,
            MiscFlags: 0,
            StructureByteStride: 0
        };
        
        let sub_data = d3d11::D3D11_SUBRESOURCE_DATA {
            pSysMem: data,
            SysMemPitch: 0,
            SysMemSlicePitch: 0
        };
        
        let hr = unsafe {d3d11_cx.device.CreateBuffer(&buffer_desc, &sub_data, &mut buffer as *mut *mut _)};
        if winerror::SUCCEEDED(hr) {
            self.last_size = len_slots;
            self.buffer = Some(unsafe {ComPtr::from_raw(buffer as *mut _)});
        }
        else {
            panic!("Buffer create failed {}", len_slots);
        }
    }
    
    pub fn update_with_u32_index_data(&mut self, d3d11_cx: &D3d11Cx, data: &[u32]) {
        self.update_with_data(d3d11_cx, d3d11::D3D11_BIND_INDEX_BUFFER, data.len(), data.as_ptr() as *const _);
    }
    
    pub fn update_with_f32_vertex_data(&mut self, d3d11_cx: &D3d11Cx, data: &[f32]) {
        self.update_with_data(d3d11_cx, d3d11::D3D11_BIND_VERTEX_BUFFER, data.len(), data.as_ptr() as *const _);
    }
    
    pub fn update_with_f32_constant_data(&mut self, d3d11_cx: &D3d11Cx, data: &mut Vec<f32>) {
        let mut buffer = ptr::null_mut();
        
        if (data.len() & 3) != 0 { // we have to align the data at the end
            let steps = 4 - (data.len() & 3);
            for _ in 0..steps {
                data.push(0.0);
            };
        }
        let sub_data = d3d11::D3D11_SUBRESOURCE_DATA {
            pSysMem: data.as_ptr() as *const _,
            SysMemPitch: 0,
            SysMemSlicePitch: 0
        };
        let len_slots = data.len();
        
        let buffer_desc = d3d11::D3D11_BUFFER_DESC {
            Usage: d3d11::D3D11_USAGE_DEFAULT,
            ByteWidth: (len_slots * 4) as u32,
            BindFlags: d3d11::D3D11_BIND_CONSTANT_BUFFER,
            CPUAccessFlags: 0,
            MiscFlags: 0,
            StructureByteStride: 0
        };
        
        let hr = unsafe {d3d11_cx.device.CreateBuffer(&buffer_desc, &sub_data, &mut buffer as *mut *mut _)};
        if winerror::SUCCEEDED(hr) {
            // println!("Buffer create OK! {} {}",  len_bytes, bind_flags);
            self.last_size = len_slots;
            self.buffer = Some(unsafe {ComPtr::from_raw(buffer as *mut _)});
        }
        else {
            panic!("Buffer create failed {}", len_slots);
        }
    }
}

pub const MAPPED_TEXTURE_BUFFER_COUNT: usize = 4;

#[derive(Default, Clone)]
pub struct CxPlatformTextureResource {
    map_ptr: Option<*mut c_void>,
    locked: bool,
    user_data: Option<usize>,
    width: usize,
    height: usize,
    slots_per_pixel: usize,
    texture: Option<ComPtr<d3d11::ID3D11Texture2D>>,
    shader_resource: Option<ComPtr<d3d11::ID3D11ShaderResourceView>>,
    d3d11_resource: Option<ComPtr<d3d11::ID3D11Resource>>
}

#[derive(Default)]
pub struct CxPlatformTextureMapped {
    read: usize,
    write: usize,
    repaint_read: Option<usize>,
    repaint_id: u64,
    textures: [CxPlatformTextureResource; MAPPED_TEXTURE_BUFFER_COUNT],
}

#[derive(Default)]
pub struct CxPlatformTexture {
    single: CxPlatformTextureResource,
    mapped: Mutex<CxPlatformTextureMapped>,
    render: Option<D3d11RenderTarget>
}

#[derive(Default, Clone, Debug)]
pub struct CxPlatformPass {
}


use std::process::{Command, Child, Stdio};

pub fn spawn_process_command(cmd: &str, args: &[&str], current_dir: &str) -> Result<Child, std::io::Error> {
    Command::new(cmd)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(current_dir)
        .spawn()
}
