use crate::cx_windows::*;
use crate::cx::*;
use crate::cx_desktop::*;

use winapi::shared::guiddef::GUID;
use winapi::shared::minwindef:: {TRUE, FALSE, UINT};
use winapi::shared:: {dxgi, dxgi1_2, dxgitype, dxgiformat, winerror};
use winapi::um:: {d3d11, d3d11sdklayers, d3dcommon, d3dcompiler};
use winapi::um::unknwnbase::IUnknown;
use winapi::Interface;
use wio::com::ComPtr;
use std::mem;
use std::ptr;
use std::ffi;

impl Cx {
    
    pub fn exec_draw_list(&mut self, draw_list_id: usize, d3d11: &D3d11) {
        
        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
        let draw_calls_len = self.draw_lists[draw_list_id].draw_calls_len;
        for draw_call_id in 0..draw_calls_len {
            let sub_list_id = self.draw_lists[draw_list_id].draw_calls[draw_call_id].sub_list_id;
            if sub_list_id != 0 {
                self.exec_draw_list(sub_list_id, d3d11);
            }
            else {
                let draw_list = &mut self.draw_lists[draw_list_id];
                draw_list.set_clipping_uniforms();
                draw_list.platform.uni_dl.update_with_f32_constant_data(d3d11, &mut draw_list.uniforms);
                let draw_call = &mut draw_list.draw_calls[draw_call_id];
                let sh = &self.shaders[draw_call.shader_id];
                let shc = &self.compiled_shaders[draw_call.shader_id];
                
                if draw_call.instance_dirty {
                    draw_call.instance_dirty = false;
                    // update the instance buffer data
                    draw_call.platform.inst_vbuf.update_with_f32_vertex_data(d3d11, &draw_call.instance);
                }
                if draw_call.uniforms_dirty {
                    draw_call.uniforms_dirty = false;
                    draw_call.platform.uni_dr.update_with_f32_constant_data(d3d11, &mut draw_call.uniforms);
                }
                
                let instances = (draw_call.instance.len() / shc.instance_slots) as usize;
                d3d11.set_shaders(&shc.vertex_shader, &shc.pixel_shader);
                
                d3d11.set_primitive_topology();
                
                d3d11.set_input_layout(&shc.input_layout);
                
                d3d11.set_index_buffer(&shc.geom_ibuf);
                
                d3d11.set_vertex_buffers(&shc.geom_vbuf, shc.geometry_slots, &draw_call.platform.inst_vbuf, shc.instance_slots);

                d3d11.set_constant_buffers(&self.platform.uni_cx, &draw_list.platform.uni_dl, &draw_call.platform.uni_dr);
                
                d3d11.draw_indexed_instanced(sh.geometry_indices.len(), instances);
            }
        }
    }
    
    pub fn repaint(&mut self, d3d11: &D3d11) {
        self.prepare_frame();
        
        self.platform.uni_cx.update_with_f32_constant_data(&d3d11, &mut self.uniforms);
        
        d3d11.set_viewport(&self.window_geom);
        
        d3d11.clear_render_target_view(self.clear_color);
        
        d3d11.clear_depth_stencil_view();
        
        self.exec_draw_list(0, d3d11);
        
        d3d11.present();
    }
    
    fn resize_layer_to_turtle(&mut self) {
    }
    
    pub fn event_loop<F>(&mut self, mut event_handler: F)
    where F: FnMut(&mut Cx, &mut Event),
    {
        self.feature = "dx11".to_string();
        
        let mut windows_window = WindowsWindow {..Default::default()};
        
        let mut root_view = View::<NoScrollBar> {
            ..Style::style(self)
        };
        
        windows_window.init(&self.title);
        
        self.window_geom = windows_window.get_window_geom();
        
        let d3d11 = D3d11::init(&windows_window, &self.window_geom);
        if let Err(msg) = d3d11 {
            panic!("Cannot initialize d3d11 {}", msg);
        }
        let d3d11 = d3d11.unwrap();
        
        self.hlsl_compile_all_shaders(&d3d11);
        
        self.load_binary_deps_from_file();
        
        self.call_event_handler(&mut event_handler, &mut Event::Construct);
        
        self.redraw_area(Area::All);
        
        while self.running {
            //println!("{}{} ",self.playing_anim_areas.len(), self.redraw_areas.len());
            windows_window.poll_events(
                self.playing_anim_areas.len() == 0 && self.redraw_areas.len() == 0 && self.next_frame_callbacks.len() == 0,
                | _events | {
                }
            );
            
            if self.playing_anim_areas.len() != 0 {
                let time = windows_window.time_now();
                // keeps the error as low as possible
                self.call_animation_event(&mut event_handler, time);
            }
            
            if self.next_frame_callbacks.len() != 0 {
                let time = windows_window.time_now();
                // keeps the error as low as possible
                self.call_frame_event(&mut event_handler, time);
            }
            
            self.call_signals_before_draw(&mut event_handler);
            
            // call redraw event
            if self.redraw_areas.len()>0 {
                //let time_start = cocoa_window.time_now();
                self.call_draw_event(&mut event_handler, &mut root_view);
                self.paint_dirty = true;
                //let time_end = cocoa_window.time_now();
                //println!("Redraw took: {}", (time_end - time_start));
            }
            
            self.process_desktop_file_read_requests(&mut event_handler);
            
            self.call_signals_after_draw(&mut event_handler);
            
            // set a cursor
            if !self.down_mouse_cursor.is_none() {
                windows_window.set_mouse_cursor(self.down_mouse_cursor.as_ref().unwrap().clone())
            }
            else if !self.hover_mouse_cursor.is_none() {
                windows_window.set_mouse_cursor(self.hover_mouse_cursor.as_ref().unwrap().clone())
            }
            else {
                windows_window.set_mouse_cursor(MouseCursor::Default)
            }
            
            if let Some(set_ime_position) = self.platform.set_ime_position {
                self.platform.set_ime_position = None;
                windows_window.ime_spot = set_ime_position;
            }
            
            if let Some(window_position) = self.platform.set_window_position {
                self.platform.set_window_position = None;
                windows_window.set_position(window_position);
                self.window_geom = windows_window.get_window_geom();
            }
            
            if let Some(window_outer_size) = self.platform.set_window_outer_size {
                self.platform.set_window_outer_size = None;
                windows_window.set_outer_size(window_outer_size);
                self.window_geom = windows_window.get_window_geom();
                self.resize_layer_to_turtle();
            }
            
            while self.platform.start_timer.len()>0 {
                let (timer_id, interval, repeats) = self.platform.start_timer.pop().unwrap();
                windows_window.start_timer(timer_id, interval, repeats);
            }
            
            while self.platform.stop_timer.len()>0 {
                let timer_id = self.platform.stop_timer.pop().unwrap();
                windows_window.stop_timer(timer_id);
            }
            
            // repaint everything if we need to
            if self.paint_dirty {
                self.paint_dirty = false;
                self.repaint_id += 1;
                self.repaint(&d3d11);
            }
        }
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
        WindowsWindow::post_signal(signal.signal_id, value);
    }
}

#[derive(Default)]
pub struct CxPlatform {
    pub uni_cx: D3d11Buffer,
    pub post_id: u64,
    pub set_window_position: Option<Vec2>,
    pub set_window_outer_size: Option<Vec2>,
    pub set_ime_position: Option<Vec2>,
    pub start_timer: Vec<(u64, f64, bool)>,
    pub stop_timer: Vec<(u64)>,
    pub text_clipboard_response: Option<String>,
    pub desktop: CxDesktop,
}

#[derive(Clone)]
pub struct D3d11 {
    pub device: ComPtr<d3d11::ID3D11Device>,
    pub context: ComPtr<d3d11::ID3D11DeviceContext>,
    pub render_target_view: ComPtr<d3d11::ID3D11RenderTargetView>,
    pub depth_stencil_view: ComPtr<d3d11::ID3D11DepthStencilView>,
    pub swap_chain: ComPtr<dxgi1_2::IDXGISwapChain1>,
    pub raster_state: ComPtr<d3d11::ID3D11RasterizerState>,
    pub blend_state: ComPtr<d3d11::ID3D11BlendState>,
}

impl D3d11 {
    
    fn init(windows_window: &WindowsWindow, window_geom:&WindowGeom) -> Result<D3d11, winerror::HRESULT> {
        let factory = D3d11::create_dxgi_factory1(&dxgi1_2::IDXGIFactory2::uuidof()) ?;
        
        let adapter = D3d11::enum_adapters(&factory) ?;
        
        let (device, context) = D3d11::create_d3d11_device(&adapter) ?;
        
        let swap_chain = D3d11::create_swap_chain_for_hwnd(window_geom, &windows_window, &factory, &device) ?;
        
        let render_target_view = D3d11::create_render_target_view(&device, &swap_chain) ?;
        
        let depth_stencil_buffer = D3d11::create_depth_stencil_buffer(window_geom, &device) ?;
        
        let depth_stencil_state = D3d11::create_depth_stencil_state(&device) ?;
        
        unsafe {context.OMSetDepthStencilState(depth_stencil_state.as_raw() as *mut _, 1)}
        
        let depth_stencil_view = D3d11::create_depth_stencil_view(&depth_stencil_buffer, &device) ?;
        
        //unsafe {context.OMSetDepthStencilState(depth_stencil_state.as_raw() as *mut _, 1)}
        
        unsafe {context.OMSetRenderTargets(1, [render_target_view.as_raw() as *mut _].as_ptr(), depth_stencil_view.as_raw() as *mut _)}
        
        let raster_state = D3d11::create_raster_state(&device) ?;
        
        unsafe {context.RSSetState(raster_state.as_raw() as *mut _)}
        
        let blend_state = D3d11::create_blend_state(&device) ?;
        
        let blend_factor = [0.,0.,0.,0.];
        unsafe {context.OMSetBlendState(blend_state.as_raw() as *mut _,&blend_factor, 0xffffffff)}
        
        
        Ok(D3d11 {
            device: device,
            context: context,
            render_target_view: render_target_view,
            depth_stencil_view: depth_stencil_view,
            swap_chain: swap_chain,
            raster_state: raster_state,
            blend_state:blend_state
        })
    }
    
    fn set_viewport(&self, window_geom:&WindowGeom) {
        let mut viewport = d3d11::D3D11_VIEWPORT {
            Width: window_geom.inner_size.x * window_geom.dpi_factor,
            Height: window_geom.inner_size.y * window_geom.dpi_factor,
            MinDepth: 0.,
            MaxDepth: 1.,
            TopLeftX: 0.0,
            TopLeftY: 0.0
        };
        
        unsafe {self.context.RSSetViewports(1, &viewport)}
        
    }
    
    fn clear_render_target_view(&self, color: Color) {
        let color = [color.r, color.g, color.b, color.a];
        unsafe {self.context.ClearRenderTargetView(self.render_target_view.as_raw() as *mut _, &color)}
    }
    
    fn clear_depth_stencil_view(&self) {
        unsafe {self.context.ClearDepthStencilView(self.depth_stencil_view.as_raw() as *mut _, d3d11::D3D11_CLEAR_DEPTH, 1.0, 0)}
    }
    
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
        unsafe {self.context.RSSetState(self.raster_state.as_raw() as *mut _)};
        
        unsafe {self.context.DrawIndexedInstanced(
            num_vertices as u32,
            num_instances as u32,
            0,
            0,
            0
        )};
    }
    
    fn present(&self) {
        unsafe {self.swap_chain.Present(1, 0)};
    }
    
    pub fn compile_shader(&self, stage: &str, entry: &[u8], shader: &[u8]) -> Result<ComPtr<d3dcommon::ID3DBlob>,
    SlErr> {
        
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
    
    pub fn create_input_layout(&self, vs: &ComPtr<d3dcommon::ID3DBlob>, layout_desc: &Vec<d3d11::D3D11_INPUT_ELEMENT_DESC>) -> Result<ComPtr<d3d11::ID3D11InputLayout>,
    SlErr> {
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
    
    pub fn create_pixel_shader(&self, ps: &ComPtr<d3dcommon::ID3DBlob>) -> Result<ComPtr<d3d11::ID3D11PixelShader>,
    SlErr> {
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
    
    
    pub fn create_vertex_shader(&self, vs: &ComPtr<d3dcommon::ID3DBlob>) -> Result<ComPtr<d3d11::ID3D11VertexShader>,
    SlErr> {
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
    
    
    fn create_raster_state(device: &ComPtr<d3d11::ID3D11Device>) -> Result<ComPtr<d3d11::ID3D11RasterizerState>,
    winerror::HRESULT> {
        let mut raster_state = ptr::null_mut();
        let mut raster_desc = d3d11::D3D11_RASTERIZER_DESC {
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
        let hr = unsafe {device.CreateRasterizerState(&raster_desc, &mut raster_state as *mut *mut _)};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(raster_state as *mut _)})
        }
        else {
            Err(hr)
        }
    }
    
    fn create_blend_state(device: &ComPtr<d3d11::ID3D11Device>) -> Result<ComPtr<d3d11::ID3D11BlendState>,
    winerror::HRESULT> {
        let mut blend_state = ptr::null_mut();
        let mut blend_desc: d3d11::D3D11_BLEND_DESC = unsafe {mem::zeroed()};
        blend_desc.AlphaToCoverageEnable = TRUE;
        blend_desc.RenderTarget[0] = d3d11::D3D11_RENDER_TARGET_BLEND_DESC{
            BlendEnable:TRUE,
            SrcBlend:d3d11::D3D11_BLEND_ONE,
            SrcBlendAlpha:d3d11::D3D11_BLEND_ONE,
            DestBlend:d3d11::D3D11_BLEND_INV_SRC_ALPHA,
            DestBlendAlpha:d3d11::D3D11_BLEND_INV_SRC_ALPHA,
            BlendOp:d3d11::D3D11_BLEND_OP_ADD,
            BlendOpAlpha:d3d11::D3D11_BLEND_OP_ADD,
            RenderTargetWriteMask:d3d11::D3D11_COLOR_WRITE_ENABLE_ALL as u8,
        };
        let hr = unsafe{device.CreateBlendState(&blend_desc, &mut blend_state as *mut *mut _)};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(blend_state as *mut _)})
        }
        else {
            Err(hr)
        }
    }
    
    fn create_depth_stencil_buffer(window_geom:&WindowGeom, device: &ComPtr<d3d11::ID3D11Device>) -> Result<ComPtr<d3d11::ID3D11Texture2D>,
    winerror::HRESULT> {
        let mut depth_stencil_buffer = ptr::null_mut();
        let mut texture2d_desc = d3d11::D3D11_TEXTURE2D_DESC {
            Width: (window_geom.inner_size.x) as u32,
            Height: (window_geom.inner_size.y) as u32,
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
        let hr = unsafe {device.CreateTexture2D(&texture2d_desc, ptr::null_mut(), &mut depth_stencil_buffer as *mut *mut _)};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(depth_stencil_buffer as *mut _)})
        }
        else {
            Err(hr)
        }
    }
    
    fn create_depth_stencil_view(buffer: &ComPtr<d3d11::ID3D11Texture2D>, device: &ComPtr<d3d11::ID3D11Device>) -> Result<ComPtr<d3d11::ID3D11DepthStencilView>,
    winerror::HRESULT> {
        let mut depth_stencil_view = ptr::null_mut();
        let mut dsv_desc: d3d11::D3D11_DEPTH_STENCIL_VIEW_DESC = unsafe {mem::zeroed()};
        dsv_desc.Format = dxgiformat::DXGI_FORMAT_D24_UNORM_S8_UINT;
        dsv_desc.ViewDimension = d3d11::D3D11_DSV_DIMENSION_TEXTURE2D;
        *unsafe {dsv_desc.u.Texture2D_mut()} = d3d11::D3D11_TEX2D_DSV {
            MipSlice: 0,
        };
        let hr = unsafe {device.CreateDepthStencilView(buffer.as_raw() as *mut _, &dsv_desc, &mut depth_stencil_view as *mut *mut _)};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(depth_stencil_view as *mut _)})
        }
        else {
            Err(hr)
        }
    }
    
    fn create_depth_stencil_state(device: &ComPtr<d3d11::ID3D11Device>) -> Result<ComPtr<d3d11::ID3D11DepthStencilState>,
    winerror::HRESULT> {
        let mut depth_stencil_state = ptr::null_mut();
        let mut ds_desc = d3d11::D3D11_DEPTH_STENCIL_DESC {
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
        
        let hr = unsafe {device.CreateDepthStencilState(&ds_desc, &mut depth_stencil_state as *mut *mut _)};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(depth_stencil_state as *mut _)})
        }
        else {
            Err(hr)
        }
    }
    
    fn create_render_target_view(device: &ComPtr<d3d11::ID3D11Device>, swap_chain: &ComPtr<dxgi1_2::IDXGISwapChain1>) -> Result<ComPtr<d3d11::ID3D11RenderTargetView>,
    winerror::HRESULT> {
        let texture = D3d11::get_swap_texture(swap_chain) ?;
        let mut render_target_view = ptr::null_mut();
        let hr = unsafe {device.CreateRenderTargetView(
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
    
    fn get_swap_texture(swap_chain: &ComPtr<dxgi1_2::IDXGISwapChain1>) -> Result<ComPtr<d3d11::ID3D11Texture2D>,
    winerror::HRESULT> {
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
    
    fn create_swap_chain_for_hwnd(window_geom:&WindowGeom, windows_window: &WindowsWindow, factory: &ComPtr<dxgi1_2::IDXGIFactory2>, device: &ComPtr<d3d11::ID3D11Device>) -> Result<ComPtr<dxgi1_2::IDXGISwapChain1>,
    winerror::HRESULT> {
        let mut swap_chain1 = ptr::null_mut();
        
        let sc_desc = dxgi1_2::DXGI_SWAP_CHAIN_DESC1 {
            AlphaMode: dxgi1_2::DXGI_ALPHA_MODE_IGNORE,
            BufferCount: 2,
            Width: (window_geom.inner_size.x) as u32,
            Height: (window_geom.inner_size.y ) as u32,
            Format: dxgiformat::DXGI_FORMAT_B8G8R8A8_UNORM,
            Flags: 0,
            BufferUsage: dxgitype::DXGI_USAGE_RENDER_TARGET_OUTPUT,
            SampleDesc: dxgitype::DXGI_SAMPLE_DESC {Count: 1, Quality: 0,},
            Scaling: dxgi1_2::DXGI_SCALING_STRETCH,
            Stereo: FALSE,
            SwapEffect: dxgi::DXGI_SWAP_EFFECT_FLIP_DISCARD,
        };
        
        let hr = unsafe {factory.CreateSwapChainForHwnd(
            device.as_raw() as *mut _,
            windows_window.hwnd.unwrap(),
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
    
    fn create_d3d11_device(adapter: &ComPtr<dxgi::IDXGIAdapter>) -> Result<(ComPtr<d3d11::ID3D11Device>, ComPtr<d3d11::ID3D11DeviceContext>),
    winerror::HRESULT> {
        let mut device = ptr::null_mut();
        let mut device_context = ptr::null_mut();
        let hr = unsafe {d3d11::D3D11CreateDevice(
            adapter.as_raw() as *mut _,
            d3dcommon::D3D_DRIVER_TYPE_UNKNOWN,
            ptr::null_mut(),
            0,//d3dcommonLLD3D11_CREATE_DEVICE_DEBUG,
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
    
    fn create_dxgi_factory1(guid: &GUID) -> Result<ComPtr<dxgi1_2::IDXGIFactory2>,
    winerror::HRESULT> {
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
    
    fn enum_adapters(factory: &ComPtr<dxgi1_2::IDXGIFactory2>) -> Result<ComPtr<dxgi::IDXGIAdapter>,
    winerror::HRESULT> {
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
    
    
    /*
     fn enum_outputs(adapter:&ComPtr<dxgi::IDXGIAdapter>) -> Result<ComPtr<dxgi::IDXGIOutput>,
    winerror::HRESULT> {
        let mut output: *mut IUnknown = ptr::null_mut();
        let hr = unsafe {adapter.EnumOutputs(0, &mut output as *mut *mut _ as *mut *mut _)};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(output as *mut _)})
        }
        else {
            Err(hr)
        }
    }*/
}

#[derive(Clone, Default)]
pub struct DrawListPlatform {
    pub uni_dl: D3d11Buffer
}

#[derive(Default, Clone, Debug)]
pub struct DrawCallPlatform {
    pub uni_dr: D3d11Buffer,
    pub inst_vbuf: D3d11Buffer
}

#[derive(Default, Clone, Debug)]
pub struct D3d11Buffer {
    pub last_size: usize,
    pub buffer: Option<ComPtr<d3d11::ID3D11Buffer>>
}

impl D3d11Buffer {
    pub fn update_with_data(&mut self, d3d11: &D3d11, bind_flags: u32, len_slots: usize, data: *const std::ffi::c_void) {
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
        
        let hr = unsafe {d3d11.device.CreateBuffer(&buffer_desc, &sub_data, &mut buffer as *mut *mut _)};
        if winerror::SUCCEEDED(hr) {
            self.last_size = len_slots;
            self.buffer = Some(unsafe {ComPtr::from_raw(buffer as *mut _)});
        }
        else {
            panic!("Buffer create failed");
            self.last_size = 0;
            self.buffer = None
        }
    }
    
    pub fn update_with_u32_index_data(&mut self, d3d11: &D3d11, data: &[u32]) {
        self.update_with_data(d3d11, d3d11::D3D11_BIND_INDEX_BUFFER, data.len(), data.as_ptr() as *const _);
    }
    
    pub fn update_with_f32_vertex_data(&mut self, d3d11: &D3d11, data: &[f32]) {
        self.update_with_data(d3d11, d3d11::D3D11_BIND_VERTEX_BUFFER, data.len(), data.as_ptr() as *const _);
    }
    
    pub fn update_with_f32_constant_data(&mut self, d3d11: &D3d11, data: &mut Vec<f32>) {
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
        
        let hr = unsafe {d3d11.device.CreateBuffer(&buffer_desc, &sub_data, &mut buffer as *mut *mut _)};
        if winerror::SUCCEEDED(hr) {
            // println!("Buffer create OK! {} {}",  len_bytes, bind_flags);
            self.last_size = len_slots;
            self.buffer = Some(unsafe {ComPtr::from_raw(buffer as *mut _)});
        }
        else {
            panic!("Buffer create failed");
            self.last_size = 0;
            self.buffer = None
        }
    }
}

#[derive(Default, Clone)]
pub struct Texture2D {
    pub texture_id: usize,
    pub dirty: bool,
    pub image: Vec<u32>,
    pub width: usize,
    pub height: usize,
    // pub dx11texture: Option<metal::Texture>
}

impl Texture2D {
    pub fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        self.image.resize((width * height) as usize, 0);
        self.dirty = true;
    }
    
    pub fn upload_to_device(&mut self) {
        self.dirty = false;
    }
}