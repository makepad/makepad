use crate::{
    makepad_shader_compiler::generate_hlsl,
    makepad_math::*,
    os::{
        windows::win32_app::{TRUE, FALSE,},
        windows::win32_window::Win32Window,
    },
    texture::Texture,
    draw_list::DrawListId,
    event::WindowGeom,
    cx::Cx,
    draw_shader::CxDrawShaderMapping,
    pass::{PassClearColor, PassClearDepth, PassId},
    window::WindowId,
    texture::{ 
        TextureFormat,
        TexturePixel,
        TextureId,
        CxTexture
    },  
    windows::{
        core::{
            PCSTR,
            ComInterface,
            Interface,
        },
        Win32::{
            Foundation::{
                HINSTANCE,
                HANDLE,
                S_FALSE,
            },
            Graphics::{
                Direct3D11::{
                    D3D11_BIND_INDEX_BUFFER,
                    D3D11_BIND_VERTEX_BUFFER,
                    D3D11_VIEWPORT,
                    D3D11_BUFFER_DESC,
                    D3D11_USAGE_DEFAULT,
                    D3D11_BIND_CONSTANT_BUFFER,
                    D3D11_RESOURCE_MISC_FLAG,
                    D3D11_SUBRESOURCE_DATA,
                    D3D11_CREATE_DEVICE_FLAG,
                    D3D11_SDK_VERSION,
                    D3D11_BIND_FLAG,
                    D3D11_BIND_SHADER_RESOURCE,
                    D3D11_TEXTURE2D_DESC,
                    D3D11_BIND_RENDER_TARGET,
                    D3D11_BIND_DEPTH_STENCIL,
                    D3D11_DEPTH_STENCIL_DESC,
                    D3D11_DEPTH_WRITE_MASK_ALL,
                    D3D11_COMPARISON_LESS_EQUAL,
                    D3D11_DEPTH_STENCILOP_DESC,
                    D3D11_STENCIL_OP_REPLACE,
                    D3D11_COMPARISON_ALWAYS,
                    D3D11_DEPTH_STENCIL_VIEW_DESC,
                    D3D11_DSV_DIMENSION_TEXTURE2D,
                    D3D11_CLEAR_DEPTH,
                    D3D11_CLEAR_STENCIL,
                    D3D11_BLEND_DESC,
                    D3D11_RENDER_TARGET_BLEND_DESC,
                    D3D11_BLEND_ONE,
                    D3D11_BLEND_INV_SRC_ALPHA,
                    D3D11_BLEND_OP_ADD,
                    D3D11_COLOR_WRITE_ENABLE_ALL,
                    D3D11_RASTERIZER_DESC,
                    D3D11_CULL_NONE,
                    D3D11_FILL_SOLID,
                    D3D11_INPUT_ELEMENT_DESC,
                    D3D11_INPUT_PER_VERTEX_DATA,
                    D3D11_INPUT_PER_INSTANCE_DATA,
                    D3D11_MAPPED_SUBRESOURCE,
                    D3D11_USAGE_DYNAMIC,
                    D3D11_CPU_ACCESS_WRITE,
                    D3D11_MAP_WRITE_DISCARD,
                    D3D11_QUERY_DESC,
                    D3D11_QUERY_EVENT,
                    ID3D11Device,
                    ID3D11DeviceContext,
                    ID3D11RenderTargetView,
                    ID3D11Texture2D,
                    ID3D11ShaderResourceView,
                    ID3D11DepthStencilView,
                    ID3D11BlendState,
                    ID3D11RasterizerState,
                    ID3D11DepthStencilState,
                    ID3D11PixelShader,
                    ID3D11VertexShader,
                    ID3D11InputLayout,
                    ID3D11Buffer,
                    D3D11CreateDevice,
                    ID3D11Resource,
                    ID3D11Query,
                },
                Direct3D::{
                    Fxc::D3DCompile,
                    ID3DBlob,
                    D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
                    D3D_DRIVER_TYPE_UNKNOWN,
                    D3D_FEATURE_LEVEL_11_0,
                },
                Dxgi::{
                    IDXGIFactory2,
                    IDXGIResource,
                    IDXGISwapChain1,
                    CreateDXGIFactory2,
                    DXGI_SWAP_CHAIN_DESC1,
                    DXGI_USAGE_RENDER_TARGET_OUTPUT,
                    DXGI_SCALING_NONE,
                    DXGI_SWAP_EFFECT_FLIP_DISCARD,
                    DXGI_RGBA,
                    Common::{
                        DXGI_FORMAT,
                        DXGI_ALPHA_MODE_IGNORE,
                        DXGI_FORMAT_R8_UNORM, 
                        DXGI_FORMAT_R8G8_UNORM,
                        DXGI_FORMAT_B8G8R8A8_UNORM,
                        DXGI_SAMPLE_DESC,
                        DXGI_FORMAT_R32G32B32A32_FLOAT,
                        DXGI_FORMAT_R16_FLOAT, 
                        //DXGI_FORMAT_D32_FLOAT_S8X 24_UINT,
                        DXGI_FORMAT_D32_FLOAT,
                        DXGI_FORMAT_R32_UINT,
                        DXGI_FORMAT_R32_FLOAT,
                        DXGI_FORMAT_R32G32_FLOAT,
                        DXGI_FORMAT_R32G32B32_FLOAT,
                    },
                },
            },
        },
    },
};

impl Cx {
    
    fn render_view(
        &mut self,
        pass_id: PassId,
        draw_list_id: DrawListId,
        zbias: &mut f32,
        zbias_step: f32,
        d3d11_cx: &D3d11Cx
    ) {
        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
        let draw_items_len = self.draw_lists[draw_list_id].draw_items.len();
        //self.views[view_id].set_clipping_uniforms();
        self.draw_lists[draw_list_id].uniform_view_transform(&Mat4::identity());
        
        {
            let draw_list = &mut self.draw_lists[draw_list_id];
            draw_list.os.view_uniforms.update_with_f32_constant_data(d3d11_cx, draw_list.draw_list_uniforms.as_slice());
        }
        
        for draw_item_id in 0..draw_items_len {
            if let Some(sub_list_id) = self.draw_lists[draw_list_id].draw_items[draw_item_id].kind.sub_list() {
                self.render_view(
                    pass_id,
                    sub_list_id,
                    zbias,
                    zbias_step,
                    d3d11_cx,
                );
            }
            else {
                let draw_list = &mut self.draw_lists[draw_list_id];
                let draw_item = &mut draw_list.draw_items[draw_item_id];
                let draw_call = if let Some(draw_call) = draw_item.kind.draw_call_mut() {
                    draw_call
                } else {
                    continue;
                };
                let sh = &self.draw_shaders[draw_call.draw_shader.draw_shader_id];
                if sh.os_shader_id.is_none() { // shader didnt compile somehow
                    continue;
                }
                let shp = &self.draw_shaders.os_shaders[sh.os_shader_id.unwrap()];
                
                if draw_call.instance_dirty {
                    draw_call.instance_dirty = false;
                    if draw_item.instances.as_ref().unwrap().len() == 0 {
                        continue;
                    }
                    // update the instance buffer data
                    draw_item.os.inst_vbuf.update_with_f32_vertex_data(d3d11_cx, draw_item.instances.as_ref().unwrap());
                }
                
                // update the zbias uniform if we have it.
                draw_call.draw_uniforms.set_zbias(*zbias);
                *zbias += zbias_step;
                
                draw_item.os.draw_uniforms.update_with_f32_constant_data(d3d11_cx, draw_call.draw_uniforms.as_slice());
                
                if draw_call.uniforms_dirty {
                    draw_call.uniforms_dirty = false;
                    if draw_call.user_uniforms.len() != 0 {
                        draw_item.os.user_uniforms.update_with_f32_constant_data(d3d11_cx, &mut draw_call.user_uniforms);
                    }
                }
                
                let instances = (draw_item.instances.as_ref().unwrap().len() / sh.mapping.instances.total_slots) as u64;
                
                if instances == 0 {
                    continue;
                }
                
                let geometry_id = if let Some(geometry_id) = draw_call.geometry_id {geometry_id}
                else {
                    continue;
                };
                
                let geometry = &mut self.geometries[geometry_id];
                
                if geometry.dirty {
                    geometry.os.geom_ibuf.update_with_u32_index_data(d3d11_cx, &geometry.indices);
                    geometry.os.geom_vbuf.update_with_f32_vertex_data(d3d11_cx, &geometry.vertices);
                    geometry.dirty = false;
                }
                
                unsafe {
                    d3d11_cx.context.VSSetShader(&shp.vertex_shader, None);
                    d3d11_cx.context.PSSetShader(&shp.pixel_shader, None);
                    d3d11_cx.context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
                    d3d11_cx.context.IASetInputLayout(&shp.input_layout);
                    
                    let geom_ibuf = geometry.os.geom_ibuf.buffer.as_ref().unwrap();
                    d3d11_cx.context.IASetIndexBuffer(geom_ibuf, DXGI_FORMAT_R32_UINT, 0);
                    
                    let geom_slots = sh.mapping.geometries.total_slots;
                    let inst_slots = sh.mapping.instances.total_slots;
                    let strides = [(geom_slots * 4) as u32, (inst_slots * 4) as u32];
                    let offsets = [0u32, 0u32];
                    let buffers = [geometry.os.geom_vbuf.buffer.clone(), draw_item.os.inst_vbuf.buffer.clone()];
                    d3d11_cx.context.IASetVertexBuffers(0, 2, Some(buffers.as_ptr()), Some(strides.as_ptr()), Some(offsets.as_ptr()));
                    
                    fn buffer_slot(d3d11_cx: &D3d11Cx, index: u32, buffer: &Option<ID3D11Buffer>) {
                        unsafe {
                            if let Some(buffer) = buffer.clone() {
                                let buffers = [Some(buffer)];
                                d3d11_cx.context.VSSetConstantBuffers(index, Some(&buffers));
                                d3d11_cx.context.PSSetConstantBuffers(index, Some(&buffers));
                            }
                            else {
                                d3d11_cx.context.VSSetConstantBuffers(index, None);
                                d3d11_cx.context.PSSetConstantBuffers(index, None);
                            }
                        }
                    }
                    buffer_slot(d3d11_cx, 0, &shp.live_uniforms.buffer);
                    buffer_slot(d3d11_cx, 1, &shp.const_table_uniforms.buffer);
                    buffer_slot(d3d11_cx, 2, &draw_item.os.draw_uniforms.buffer);
                    buffer_slot(d3d11_cx, 3, &self.passes[pass_id].os.pass_uniforms.buffer);
                    buffer_slot(d3d11_cx, 4, &draw_list.os.view_uniforms.buffer);
                    buffer_slot(d3d11_cx, 5, &draw_item.os.user_uniforms.buffer);
                }
                
                for i in 0..sh.mapping.textures.len() {
                    
                    let texture_id = if let Some(texture) = &draw_call.texture_slots[i] {
                        texture.texture_id()
                    } else {
                        continue;
                    };
                    
                    let cxtexture = &mut self.textures[texture_id];
                    
                    if cxtexture.format.is_shared() {
                        cxtexture.update_shared_texture(
                            &d3d11_cx.device,
                        );
                    }
                    else if cxtexture.format.is_vec(){
                        cxtexture.update_vec_texture(
                            d3d11_cx,
                        );
                    }
                    unsafe {
                        if let Some(sr) = &cxtexture.os.shader_resource_view {
                            d3d11_cx.context.PSSetShaderResources(i as u32, Some(&[Some(sr.clone())]));
                            d3d11_cx.context.VSSetShaderResources(i as u32, Some(&[Some(sr.clone())]));
                        }
                        else {
                            d3d11_cx.context.PSSetShaderResources(i as u32, None);
                            d3d11_cx.context.VSSetShaderResources(i as u32, None);
                        }
                    }
                }
                //if self.passes[pass_id].debug{
                // println!("DRAWING {} {}", geometry.indices.len(), instances);
                //}
                unsafe {
                    d3d11_cx.context.DrawIndexedInstanced(
                        geometry.indices.len() as u32,
                        instances as u32,
                        0,
                        0,
                        0
                    )
                };
            }
        }
    }

    pub fn get_shared_handle(&self, _texture: &Texture) -> HANDLE {
        self.textures[_texture.texture_id()].os.shared_handle
    }

    pub fn setup_pass_render_targets(&mut self, pass_id: PassId, first_target: &Option<ID3D11RenderTargetView >, d3d11_cx: &D3d11Cx) {
        
        let dpi_factor = self.passes[pass_id].dpi_factor.unwrap();
        
        let pass_rect = self.get_pass_rect(pass_id, dpi_factor).unwrap();
        self.passes[pass_id].set_matrix(pass_rect.pos, pass_rect.size);
        self.passes[pass_id].paint_dirty = false;

        self.passes[pass_id].set_dpi_factor(dpi_factor);
        
        let viewport = D3D11_VIEWPORT {
            Width: (pass_rect.size.x * dpi_factor) as f32,
            Height: (pass_rect.size.y * dpi_factor) as f32,
            MinDepth: 0.,
            MaxDepth: 1.,
            TopLeftX: 0.0,
            TopLeftY: 0.0
        };
        unsafe {
            d3d11_cx.context.RSSetViewports(Some(&[viewport]));
        }
        if viewport.Width < 1.0 || viewport.Height < 1.0{
            return
        }
        // set up the color texture array
        let mut color_textures = Vec::<Option<ID3D11RenderTargetView>>::new();
        
        if let Some(render_target) = first_target {
            color_textures.push(Some(render_target.clone()));
            let color = self.passes[pass_id].clear_color;
            let color = [color.x, color.y, color.z, color.w];
            unsafe {d3d11_cx.context.ClearRenderTargetView(first_target.as_ref().unwrap(), &color)}
        }
        else {
            for color_texture in self.passes[pass_id].color_textures.iter() {
                let cxtexture = &mut self.textures[color_texture.texture.texture_id()];
                let size = pass_rect.size * dpi_factor;
                cxtexture.update_render_target(d3d11_cx, size.x as usize, size.y as usize);
                let is_initial = cxtexture.take_initial();
                let render_target = cxtexture.os.render_target_view.clone();
                color_textures.push(Some(render_target.clone().unwrap()));
                // possibly clear it
                match color_texture.clear_color {
                    PassClearColor::InitWith(color) => {
                        if is_initial {
                            let color = [color.x, color.y, color.z, color.w];
                            unsafe {d3d11_cx.context.ClearRenderTargetView(render_target.as_ref().unwrap(), &color)}
                        }
                    },
                    PassClearColor::ClearWith(color) => {
                        let color = [color.x, color.y, color.z, color.w];
                        unsafe {d3d11_cx.context.ClearRenderTargetView(render_target.as_ref().unwrap(), &color)}
                    }
                }
            }
        }
        
        // attach/clear depth buffers, if any
        if let Some(depth_texture) = &self.passes[pass_id].depth_texture {
            let cxtexture = &mut self.textures[depth_texture.texture_id()];
            let size = pass_rect.size * dpi_factor;
            cxtexture.update_depth_stencil(d3d11_cx, size.x as usize, size.y as usize);
            let is_initial = cxtexture.take_initial();
                        
            match self.passes[pass_id].clear_depth {
                PassClearDepth::InitWith(depth_clear) => {
                    if is_initial {
                        unsafe {d3d11_cx.context.ClearDepthStencilView(
                            cxtexture.os.depth_stencil_view.as_ref().unwrap(),
                            D3D11_CLEAR_DEPTH.0 as u32 | D3D11_CLEAR_STENCIL.0 as u32,
                            depth_clear,
                            0
                        )}
                    }
                },
                PassClearDepth::ClearWith(depth_clear) => {
                    unsafe {d3d11_cx.context.ClearDepthStencilView(
                        cxtexture.os.depth_stencil_view.as_ref().unwrap(),
                        D3D11_CLEAR_DEPTH.0 as u32 | D3D11_CLEAR_STENCIL.0 as u32,
                        depth_clear,
                        0
                    )}
                }
            }
            unsafe {d3d11_cx.context.OMSetRenderTargets(
                Some(&color_textures),
                None, //cxtexture.os.depth_stencil_view.as_ref().unwrap()
            )}
        }
        else {
            unsafe {d3d11_cx.context.OMSetRenderTargets(
                Some(&color_textures),
                None
            )}
        }
        
        // create depth, blend and raster states
        self.passes[pass_id].os.set_states(d3d11_cx);
        
        let cxpass = &mut self.passes[pass_id];
        
        cxpass.os.pass_uniforms.update_with_f32_constant_data(&d3d11_cx, cxpass.pass_uniforms.as_slice());
    }
    
    pub fn draw_pass_to_window(&mut self, pass_id: PassId, vsync: bool, d3d11_window: &mut D3d11Window, d3d11_cx: &D3d11Cx) {
        // let time1 = Cx::profile_time_ns();
        let draw_list_id = self.passes[pass_id].main_draw_list_id.unwrap();
        
        self.setup_pass_render_targets(pass_id, &d3d11_window.render_target_view, d3d11_cx);
        
        let mut zbias = 0.0;
        let zbias_step = self.passes[pass_id].zbias_step;
        
        self.render_view(
            pass_id,
            draw_list_id,
            &mut zbias,
            zbias_step,
            d3d11_cx
        );
        d3d11_window.present(vsync);
        if d3d11_window.first_draw {
            d3d11_window.win32_window.show();
            d3d11_window.first_draw = false;
        }
        //println!("{}", (Cx::profile_time_ns() - time1)as f64 / 1000.0);
    }
    
    pub fn draw_pass_to_texture(&mut self, pass_id: PassId,  d3d11_cx: &D3d11Cx,texture_id: TextureId) {
        // let time1 = Cx::profile_time_ns();
        let draw_list_id = self.passes[pass_id].main_draw_list_id.unwrap();
        
        let render_target_view = self.textures[texture_id].os.render_target_view.clone();
        self.setup_pass_render_targets(pass_id, &render_target_view, d3d11_cx);
        
        let mut zbias = 0.0;
        let zbias_step = self.passes[pass_id].zbias_step;
        self.render_view(
            pass_id,
            draw_list_id,
            &mut zbias,
            zbias_step,
            &d3d11_cx,
        );
    }
    
    pub fn draw_pass_to_magic_texture(&mut self, pass_id: PassId,  d3d11_cx: &D3d11Cx) {
        // let time1 = Cx::profile_time_ns();
        let draw_list_id = self.passes[pass_id].main_draw_list_id.unwrap();
        
        self.setup_pass_render_targets(pass_id, &None, d3d11_cx);
        
        let mut zbias = 0.0;
        let zbias_step = self.passes[pass_id].zbias_step;
        self.render_view(
            pass_id,
            draw_list_id,
            &mut zbias,
            zbias_step,
            &d3d11_cx,
        );
    }
    
    pub (crate) fn hlsl_compile_shaders(&mut self, d3d11_cx: &D3d11Cx) {
        for draw_shader_ptr in &self.draw_shaders.compile_set {
            if let Some(item) = self.draw_shaders.ptr_to_item.get(&draw_shader_ptr) {
                let cx_shader = &mut self.draw_shaders.shaders[item.draw_shader_id];
                let draw_shader_def = self.shader_registry.draw_shader_defs.get(&draw_shader_ptr);
                let hlsl = generate_hlsl::generate_shader(
                    draw_shader_def.as_ref().unwrap(),
                    &cx_shader.mapping.const_table,
                    &self.shader_registry
                );
                
                if cx_shader.mapping.flags.debug {
                    crate::log!("{}", hlsl);
                }
                // lets see if we have the shader already
                for (index, ds) in self.draw_shaders.os_shaders.iter().enumerate() {
                    if ds.hlsl == hlsl {
                        cx_shader.os_shader_id = Some(index);
                        break;
                    }
                }
                if cx_shader.os_shader_id.is_none() {
                    if let Some(shp) = CxOsDrawShader::new(d3d11_cx, hlsl, &cx_shader.mapping) {
                        cx_shader.os_shader_id = Some(self.draw_shaders.os_shaders.len());
                        self.draw_shaders.os_shaders.push(shp);
                    }
                }
            }
        }
        self.draw_shaders.compile_set.clear();
    }

    pub fn share_texture_for_presentable_image(
        &mut self,
        texture: &Texture,
    ) -> crate::cx_stdin::SharedPresentableImageOsHandle {
        let cxtexture = &mut self.textures[texture.texture_id()];
        cxtexture.update_shared_texture(self.os.d3d11_device.as_ref().unwrap());
        cxtexture.os.shared_handle.0 as u64
    }
}

fn texture_pixel_to_dx11_pixel(pix:&TexturePixel)->DXGI_FORMAT{
    match pix{
        TexturePixel::BGRAu8 => DXGI_FORMAT_B8G8R8A8_UNORM,
        TexturePixel::RGBAf16 => DXGI_FORMAT_R16_FLOAT,
        TexturePixel::RGBAf32 => DXGI_FORMAT_R32G32B32A32_FLOAT,
        TexturePixel::Ru8  => DXGI_FORMAT_R8_UNORM,
        TexturePixel::RGu8  => DXGI_FORMAT_R8G8_UNORM,
        TexturePixel::Rf32  => DXGI_FORMAT_R32_FLOAT,
        TexturePixel::D32 => DXGI_FORMAT_D32_FLOAT,
    }   
}

pub struct D3d11Window {
    pub window_id: WindowId,
    pub is_in_resize: bool,
    pub window_geom: WindowGeom,
    pub win32_window: Box<Win32Window>,
    pub render_target_view: Option<ID3D11RenderTargetView >,
    pub swap_texture: Option<ID3D11Texture2D >,
    pub alloc_size: DVec2,
    pub first_draw: bool,
    pub swap_chain: IDXGISwapChain1,
}

impl D3d11Window {
    pub fn new(window_id: WindowId, d3d11_cx: &D3d11Cx, inner_size: DVec2, position: Option<DVec2>, title: &str) -> D3d11Window {

        // create window, and then initialize it; this is needed because
        // GWLP_USERDATA needs to reference a stable and existing window
        let mut win32_window = Box::new(Win32Window::new(window_id, title, position));
        win32_window.init(inner_size);
        
        let wg = win32_window.get_window_geom();
        
        let sc_desc = DXGI_SWAP_CHAIN_DESC1 {
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            BufferCount: 2,
            Width: (wg.inner_size.x * wg.dpi_factor) as u32,
            Height: (wg.inner_size.y * wg.dpi_factor) as u32,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Flags: 0,
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            SampleDesc: DXGI_SAMPLE_DESC {Count: 1, Quality: 0,},
            Scaling: DXGI_SCALING_NONE,
            Stereo: FALSE,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
        };
        
        unsafe {
            let swap_chain = d3d11_cx.factory.CreateSwapChainForHwnd(
                &d3d11_cx.device,
                win32_window.hwnd,
                &sc_desc,
                None,
                None,
            ).unwrap();
            
            let swap_texture = swap_chain.GetBuffer(0).unwrap();
            let mut render_target_view = None;
            d3d11_cx.device.CreateRenderTargetView(&swap_texture, None, Some(&mut render_target_view)).unwrap();
            swap_chain.SetBackgroundColor(&mut DXGI_RGBA {
                r: 0.3,
                g: 0.3,
                b: 0.3,
                a: 1.0
            }).unwrap();
            D3d11Window {
                first_draw: true,
                is_in_resize: false,
                window_id: window_id,
                alloc_size: wg.inner_size,
                window_geom: wg,
                win32_window: win32_window,
                swap_texture: Some(swap_texture),
                render_target_view: render_target_view,
                swap_chain: swap_chain,
            }
        }
    }
    
    pub fn start_resize(&mut self) {
        self.is_in_resize = true;
    }
    
    // switch back to swapchain
    pub fn stop_resize(&mut self) {
        self.is_in_resize = false;
        self.alloc_size = DVec2::default();
    }
    
    
    pub fn resize_buffers(&mut self, d3d11_cx: &D3d11Cx) {
        if self.alloc_size == self.window_geom.inner_size {
            return
        }
        self.alloc_size = self.window_geom.inner_size;
        self.swap_texture = None;
        self.render_target_view = None;
        
        unsafe {
            let wg = &self.window_geom;
            self.swap_chain.ResizeBuffers(
                2,
                (wg.inner_size.x * wg.dpi_factor) as u32,
                (wg.inner_size.y * wg.dpi_factor) as u32,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                0
            ).unwrap();
            
            let swap_texture = self.swap_chain.GetBuffer(0).unwrap();
            let mut render_target_view = None;
            d3d11_cx.device.CreateRenderTargetView(&swap_texture, None, Some(&mut render_target_view)).unwrap();
            
            self.swap_texture = Some(swap_texture);
            self.render_target_view = render_target_view;
        }
    }
    
    pub fn present(&mut self, vsync: bool) {
        unsafe {self.swap_chain.Present(if vsync {1}else {0}, 0).unwrap()};
    }
    
}

#[derive(Clone)]
pub struct D3d11Cx {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    pub query: ID3D11Query,
    pub factory: IDXGIFactory2,
}

impl D3d11Cx {
    
    pub fn new() -> D3d11Cx {
        unsafe {
            let factory: IDXGIFactory2 = CreateDXGIFactory2(0).unwrap();
            let adapter = factory.EnumAdapters(0).unwrap();
            let mut device: Option<ID3D11Device> = None;
            let mut context: Option<ID3D11DeviceContext> = None;
            let mut query: Option<ID3D11Query> = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HINSTANCE(0),
                D3D11_CREATE_DEVICE_FLAG(0),
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context)
            ).unwrap();

            let device = device.unwrap();
            let context = context.unwrap();

            device.CreateQuery(&D3D11_QUERY_DESC {
                Query: D3D11_QUERY_EVENT,
                MiscFlags: 0,
            },Some(&mut query)).unwrap();

            let query = query.unwrap();

            D3d11Cx {
                device,
                context,
                factory,
                query,
            }
        }
    }

    pub fn start_querying(&self) {

        // QUERY_EVENT signals when rendering is complete
        unsafe { self.context.End(&self.query) };
    }

    pub fn is_gpu_done(&self) -> bool {
        let hresult = unsafe { (Interface::vtable(&self.context).GetData)(Interface::as_raw(&self.context),Interface::as_raw(&self.query),std::ptr::null_mut(),0,0) };
        hresult != S_FALSE
    }
}

#[derive(Clone, Default)]
pub struct CxOsView {
    pub view_uniforms: D3d11Buffer
}

#[derive(Default, Clone)]
pub struct CxOsDrawCall {
    pub draw_uniforms: D3d11Buffer,
    pub user_uniforms: D3d11Buffer,
    pub inst_vbuf: D3d11Buffer
}

#[derive(Default, Clone)]
pub struct D3d11Buffer {
    pub last_size: usize,
    pub buffer: Option<ID3D11Buffer>
}

impl D3d11Buffer {
    fn create_buffer_or_update(&mut self, d3d11_cx: &D3d11Cx, buffer_desc: &D3D11_BUFFER_DESC, sub_data: &D3D11_SUBRESOURCE_DATA, len_slots: usize, data: *const std::ffi::c_void) {
        if self.last_size == len_slots {
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            let p_mapped : *mut _ = &mut mapped;
            unsafe {
                d3d11_cx.context.Map(self.buffer.as_ref().unwrap(),
                        0, D3D11_MAP_WRITE_DISCARD, 0, Some(p_mapped)).unwrap();

                std::ptr::copy_nonoverlapping(data, mapped.pData, len_slots * 4);
                d3d11_cx.context.Unmap(self.buffer.as_ref().unwrap(), 0);
            }
        } else {
            self.last_size = len_slots;
            unsafe { d3d11_cx.device.CreateBuffer(buffer_desc, Some(sub_data), Some(&mut self.buffer)).unwrap() }
        }
    }

    pub fn update_with_data(&mut self, d3d11_cx: &D3d11Cx, bind_flags: D3D11_BIND_FLAG, len_slots: usize, data: *const std::ffi::c_void) {
        let buffer_desc = D3D11_BUFFER_DESC {
            Usage: D3D11_USAGE_DYNAMIC,
            ByteWidth: (len_slots * 4) as u32,
            BindFlags: bind_flags.0 as u32,
            CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
            MiscFlags: 0,
            StructureByteStride: 0
        };
        
        let sub_data = D3D11_SUBRESOURCE_DATA {
            pSysMem: data,
            SysMemPitch: 0,
            SysMemSlicePitch: 0
        };
        self.create_buffer_or_update(d3d11_cx, &buffer_desc, &sub_data, len_slots, data);
    }
    
    pub fn update_with_u32_index_data(&mut self, d3d11_cx: &D3d11Cx, data: &[u32]) {
        self.update_with_data(d3d11_cx, D3D11_BIND_INDEX_BUFFER, data.len(), data.as_ptr() as *const _);
    }
    
    pub fn update_with_f32_vertex_data(&mut self, d3d11_cx: &D3d11Cx, data: &[f32]) {
        self.update_with_data(d3d11_cx, D3D11_BIND_VERTEX_BUFFER, data.len(), data.as_ptr() as *const _);
    }
    
    pub fn update_with_f32_constant_data(&mut self, d3d11_cx: &D3d11Cx, data: &[f32]) {
        if data.len() == 0 {
            return
        }
        if (data.len() & 3) != 0 { // we have to align the data at the end
            let mut new_data = data.to_vec();
            let steps = 4 - (data.len() & 3);
            for _ in 0..steps {
                new_data.push(0.0);
            };
            return self.update_with_f32_constant_data(d3d11_cx, &new_data);
        }
        let sub_data = D3D11_SUBRESOURCE_DATA {
            pSysMem: data.as_ptr() as *const _,
            SysMemPitch: 0,
            SysMemSlicePitch: 0
        };
        let len_slots = data.len();
        
        let buffer_desc = D3D11_BUFFER_DESC {
            Usage: D3D11_USAGE_DYNAMIC,
            ByteWidth: (len_slots * 4) as u32,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
            MiscFlags: 0,
            StructureByteStride: 0
        };
        let data = unsafe { core::slice::from_raw_parts(data.as_ptr() as *const u8, std::mem::size_of::<f32>() * data.len()).as_ptr() as *const _ };
        self.create_buffer_or_update(d3d11_cx, &buffer_desc, &sub_data, len_slots, data);
    }
}


#[derive(Default)]
pub struct CxOsTexture {
    texture: Option<ID3D11Texture2D >,
    pub shared_handle: HANDLE,
    shader_resource_view: Option<ID3D11ShaderResourceView >,
    render_target_view: Option<ID3D11RenderTargetView >,
    depth_stencil_view: Option<ID3D11DepthStencilView >,
}

impl CxTexture {
    
    pub fn update_vec_texture(
        &mut self,
        d3d11_cx: &D3d11Cx,
    ) {
        // TODO maybe we can update the data instead of making a new texture?
        if self.alloc_vec(){}
        if !self.take_updated().is_empty() {
            fn get_descs(format: DXGI_FORMAT, width: usize, height: usize, bpp: usize, data: *const std::ffi::c_void)->(D3D11_SUBRESOURCE_DATA,D3D11_TEXTURE2D_DESC) {
                let sub_data = D3D11_SUBRESOURCE_DATA {
                    pSysMem: data,
                    SysMemPitch: (width * bpp) as u32,
                    SysMemSlicePitch: 0
                };
                                            
                let texture_desc = D3D11_TEXTURE2D_DESC {
                    Width: width as u32,
                    Height: height as u32,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: format,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0
                    },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                    CPUAccessFlags: 0,
                    MiscFlags: 0,
                };
                (sub_data,texture_desc)
            }
            
            let (sub_data, texture_desc) = match &self.format{
                TextureFormat::VecBGRAu8_32{width, height, data, ..}=>{
                    get_descs(DXGI_FORMAT_B8G8R8A8_UNORM, *width, *height, 4, data.as_ref().unwrap().as_ptr() as *const _)
                }
                TextureFormat::VecRGBAf32{width, height, data, ..}=>{
                    get_descs(DXGI_FORMAT_R32G32B32A32_FLOAT, *width, *height, 16, data.as_ref().unwrap().as_ptr() as *const _)
                }
                TextureFormat::VecRu8{width, height, data, ..}=>{
                    get_descs(DXGI_FORMAT_R8_UNORM, *width, *height, 1, data.as_ref().unwrap().as_ptr() as *const _)
                }
                TextureFormat::VecRGu8{width, height, data, ..}=>{
                    get_descs(DXGI_FORMAT_R8G8_UNORM, *width, *height, 1, data.as_ref().unwrap().as_ptr() as *const _)
                }
                TextureFormat::VecRf32{width, height, data, ..}=>{
                    get_descs(DXGI_FORMAT_R32_FLOAT, *width, *height, 4, data.as_ref().unwrap().as_ptr() as *const _)
                }
                _=>panic!()
            };
                                        
            let mut texture = None;
            unsafe {d3d11_cx.device.CreateTexture2D(&texture_desc, Some(&sub_data), Some(&mut texture)).unwrap()};
            let resource: ID3D11Resource = texture.clone().unwrap().cast().unwrap();
            let mut shader_resource_view = None;
            unsafe {d3d11_cx.device.CreateShaderResourceView(&resource, None, Some(&mut shader_resource_view)).unwrap()};
            self.os.texture = texture;
            self.os.shader_resource_view = shader_resource_view;
        }
    }
    
    pub fn update_render_target(
        &mut self,
        d3d11_cx: &D3d11Cx,
        width: usize,
        height: usize
    ) {
        if self.alloc_render(width, height){
            let alloc = self.alloc.as_ref().unwrap();
            let misc_flags = D3D11_RESOURCE_MISC_FLAG(0);
            let format = texture_pixel_to_dx11_pixel(&alloc.pixel);
            
            let texture_desc = D3D11_TEXTURE2D_DESC {
                Width: width as u32,
                Height: height as u32,
                MipLevels: 1,
                ArraySize: 1,
                Format: format,
                SampleDesc: DXGI_SAMPLE_DESC {Count: 1, Quality: 0},
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                CPUAccessFlags: 0,
                MiscFlags: misc_flags.0 as u32,
            };
            
            let mut texture = None;
            unsafe {d3d11_cx.device.CreateTexture2D(&texture_desc, None, Some(&mut texture)).unwrap()};
            let resource: ID3D11Resource = texture.clone().unwrap().cast().unwrap();
            let mut shader_resource_view = None;
            unsafe {d3d11_cx.device.CreateShaderResourceView(&resource, None, Some(&mut shader_resource_view)).unwrap()};
            let mut render_target_view = None;
            unsafe {d3d11_cx.device.CreateRenderTargetView(&resource, None, Some(&mut render_target_view)).unwrap()};
            
            self.os.texture = texture;
            self.os.shader_resource_view = shader_resource_view;
            self.os.render_target_view = render_target_view;
        }
    }
    
    pub fn update_depth_stencil(
        &mut self,
        d3d11_cx: &D3d11Cx,
        width: usize,
        height: usize
    ) {
        if self.alloc_depth(width, height){
            let alloc = self.alloc.as_ref().unwrap();
            let format;
            match alloc.pixel {
                TexturePixel::D32 => {
                    format = DXGI_FORMAT_D32_FLOAT;
                }
                _ => {
                    panic!("Wrong format for update_depth_stencil");
                }
            }
            let texture_desc = D3D11_TEXTURE2D_DESC {
                Width: width as u32,
                Height: height as u32,
                MipLevels: 1,
                ArraySize: 1,
                Format: format,
                SampleDesc: DXGI_SAMPLE_DESC {Count: 1, Quality: 0},
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_DEPTH_STENCIL.0 as u32, // | D3D11_BIND_SHADER_RESOURCE,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            
            let mut texture = None;
            unsafe {d3d11_cx.device.CreateTexture2D(&texture_desc, None, Some(&mut texture)).unwrap()};
            let resource: ID3D11Resource = texture.clone().unwrap().cast().unwrap();
            //let shader_resource_view = unsafe {d3d11_cx.device.CreateShaderResourceView(&texture, None).unwrap()};
            
            let dsv_desc = D3D11_DEPTH_STENCIL_VIEW_DESC {
                Format: DXGI_FORMAT_D32_FLOAT,
                ViewDimension: D3D11_DSV_DIMENSION_TEXTURE2D,
                Flags: 0,
                ..Default::default()
            };
            
            let mut depth_stencil_view = None;
            unsafe {d3d11_cx.device.CreateDepthStencilView(&resource, Some(&dsv_desc), Some(&mut depth_stencil_view)).unwrap()};
            
            self.os.depth_stencil_view = depth_stencil_view;
            self.os.texture = texture;
            self.os.shader_resource_view = None; //Some(shader_resource_view);
        }
    }
    
    fn update_shared_texture(
        &mut self,
        d3d11_device: &ID3D11Device,
    ) {
        if self.alloc_shared(){
            let alloc = self.alloc.as_ref().unwrap();

            let texture_desc = D3D11_TEXTURE2D_DESC {
                Width: alloc.width as u32,
                Height: alloc.height as u32,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                CPUAccessFlags: 0,
                MiscFlags: 2,  // D3D11_RESOURCE_MISC_SHARED
            };
            
            let mut texture = None;
            unsafe {d3d11_device.CreateTexture2D(&texture_desc, None, Some(&mut texture)).unwrap()};
            let resource: ID3D11Resource = texture.clone().unwrap().cast().unwrap();
            let mut shader_resource_view = None;
            unsafe {d3d11_device.CreateShaderResourceView(&resource, None, Some(&mut shader_resource_view)).unwrap()};

            // get IDXGIResource interface on newly created texture object
            let dxgi_resource: IDXGIResource = resource.cast().unwrap();
            //let mut dxgi_resource_ptr = None;
            //unsafe { resource.query(IDXGIResource::IID,Some(&mut dxgi_resource_ptr)).unwrap() };
            //let dxgi_resource = dxgi_resource_ptr.as_ref().unwrap().into();

            // get shared handle of this resource
            let handle = unsafe { dxgi_resource.GetSharedHandle().unwrap() };
            //log!("created new shared texture with handle {:?}",handle);

            self.os.texture = texture;
            self.os.shader_resource_view = shader_resource_view;
            self.os.shared_handle = handle;
        }
    }

    pub fn update_from_shared_handle(&mut self,
        d3d11_cx: &D3d11Cx,
        handle: HANDLE,
    ) {
        if self.alloc_shared(){
            let mut texture: Option<ID3D11Texture2D> = None;
            if let Ok(()) = unsafe { d3d11_cx.device.OpenSharedResource(handle,&mut texture) } {
    
                let resource: ID3D11Resource = texture.clone().unwrap().cast().unwrap();
                let mut shader_resource_view = None;
                unsafe {d3d11_cx.device.CreateShaderResourceView(&resource, None, Some(&mut shader_resource_view)).unwrap()};
                let mut render_target_view = None;
                unsafe {d3d11_cx.device.CreateRenderTargetView(&resource, None, Some(&mut render_target_view)).unwrap()};
                self.os.texture = texture;
                self.os.render_target_view = render_target_view;
                self.os.shader_resource_view = shader_resource_view;
            }
        }
    }
}

impl CxOsPass {
    pub fn set_states(&mut self, d3d11_cx: &D3d11Cx,) {
        
        if self.blend_state.is_none() {
            
            let mut blend_desc: D3D11_BLEND_DESC = Default::default();
            blend_desc.AlphaToCoverageEnable = FALSE;
            blend_desc.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
                BlendEnable: TRUE,
                SrcBlend: D3D11_BLEND_ONE,
                SrcBlendAlpha: D3D11_BLEND_ONE,
                DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
                DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
                BlendOp: D3D11_BLEND_OP_ADD,
                BlendOpAlpha: D3D11_BLEND_OP_ADD,
                RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
            };
            unsafe {d3d11_cx.device.CreateBlendState(&blend_desc, Some(&mut self.blend_state)).unwrap()}
        }
        
        if self.raster_state.is_none() {
            let raster_desc = D3D11_RASTERIZER_DESC {
                AntialiasedLineEnable: FALSE,
                CullMode: D3D11_CULL_NONE,
                DepthBias: 0,
                DepthBiasClamp: 0.0,
                DepthClipEnable: TRUE,
                FillMode: D3D11_FILL_SOLID,
                FrontCounterClockwise: FALSE,
                MultisampleEnable: FALSE,
                ScissorEnable: FALSE,
                SlopeScaledDepthBias: 0.0,
            };
            unsafe {d3d11_cx.device.CreateRasterizerState(&raster_desc, Some(&mut self.raster_state)).unwrap()}
        }
        
        if self.depth_stencil_state.is_none() {
            let ds_desc = D3D11_DEPTH_STENCIL_DESC {
                DepthEnable: TRUE,
                DepthWriteMask: D3D11_DEPTH_WRITE_MASK_ALL,
                DepthFunc: D3D11_COMPARISON_LESS_EQUAL,
                StencilEnable: FALSE,
                StencilReadMask: 0xff,
                StencilWriteMask: 0xff,
                FrontFace: D3D11_DEPTH_STENCILOP_DESC {
                    StencilFailOp: D3D11_STENCIL_OP_REPLACE,
                    StencilDepthFailOp: D3D11_STENCIL_OP_REPLACE,
                    StencilPassOp: D3D11_STENCIL_OP_REPLACE,
                    StencilFunc: D3D11_COMPARISON_ALWAYS,
                },
                BackFace: D3D11_DEPTH_STENCILOP_DESC {
                    StencilFailOp: D3D11_STENCIL_OP_REPLACE,
                    StencilDepthFailOp: D3D11_STENCIL_OP_REPLACE,
                    StencilPassOp: D3D11_STENCIL_OP_REPLACE,
                    StencilFunc: D3D11_COMPARISON_ALWAYS,
                },
            };
            unsafe {d3d11_cx.device.CreateDepthStencilState(&ds_desc, Some(&mut self.depth_stencil_state)).unwrap()}
        }
        
        unsafe {
            d3d11_cx.context.RSSetState(self.raster_state.as_ref().unwrap());
            let blend_factor = [0., 0., 0., 0.];
            d3d11_cx.context.OMSetBlendState(self.blend_state.as_ref().unwrap(), Some(&blend_factor), 0xffffffff);
            d3d11_cx.context.OMSetDepthStencilState(self.depth_stencil_state.as_ref().unwrap(), 0);
        }
    }
}

#[derive(Default, Clone)]
pub struct CxOsPass {
    pass_uniforms: D3d11Buffer,
    blend_state: Option<ID3D11BlendState >,
    raster_state: Option<ID3D11RasterizerState >,
    depth_stencil_state: Option<ID3D11DepthStencilState >
}

#[derive(Default, Clone)]
pub struct CxOsGeometry {
    pub geom_vbuf: D3d11Buffer,
    pub geom_ibuf: D3d11Buffer,
}

#[derive(Clone)]
pub struct CxOsDrawShader {
    pub hlsl: String,
    pub const_table_uniforms: D3d11Buffer,
    pub live_uniforms: D3d11Buffer,
    pub pixel_shader: ID3D11PixelShader,
    pub vertex_shader: ID3D11VertexShader,
    pub pixel_shader_blob: ID3DBlob,
    pub vertex_shader_blob: ID3DBlob,
    pub input_layout: ID3D11InputLayout
}

impl CxOsDrawShader {
    
    fn new(d3d11_cx: &D3d11Cx, hlsl: String, mapping: &CxDrawShaderMapping) -> Option<Self> {
        
        fn compile_shader(target: &str, entry: &str, shader: &str) -> Result<ID3DBlob, String> {
            unsafe {
                let shader_bytes = shader.as_bytes();
                let mut blob = None;
                let mut errors = None;
                if D3DCompile(
                    shader_bytes.as_ptr() as *const _,
                    shader_bytes.len(),
                    PCSTR("makepad_shader\0".as_ptr()), // sourcename
                    None, // defines
                    None, // include
                    PCSTR(entry.as_ptr()), // entry point
                    PCSTR(target.as_ptr()), // target
                    0, // flags1
                    0, // flags2
                    &mut blob,
                    Some(&mut errors)
                ).is_ok() {
                    return Ok(blob.unwrap());
                };
                let error = errors.unwrap();
                let pointer = error.GetBufferPointer();
                let size = error.GetBufferSize();
                let slice = std::slice::from_raw_parts(pointer as *const u8, size as usize);
                return Err(String::from_utf8_lossy(slice).into_owned());
            }
        }
        fn split_source(src: &str) -> String {
            let mut r = String::new();
            let split = src.split("\n");
            for (line, chunk) in split.enumerate() {
                r.push_str(&(line + 1).to_string());
                r.push_str(":");
                r.push_str(chunk);
                r.push_str("\n");
            }
            return r
        }
        
        fn slots_to_dxgi_format(slots: usize) -> DXGI_FORMAT {
            
            match slots {
                1 => DXGI_FORMAT_R32_FLOAT,
                2 => DXGI_FORMAT_R32G32_FLOAT,
                3 => DXGI_FORMAT_R32G32B32_FLOAT,
                4 => DXGI_FORMAT_R32G32B32A32_FLOAT,
                _ => panic!("slots_to_dxgi_format unsupported slotcount {}", slots)
            }
        }
        
        let vs_blob = match compile_shader("vs_5_0\0", "vertex_main\0", &hlsl) {
            Err(msg) => {
                println!("Cannot compile vertexshader\n{}\n{}", msg, split_source(&hlsl));
                return None
            },
            Ok(blob) => {
                blob
            }
        };
        
        let ps_blob = match compile_shader("ps_5_0\0", "pixel_main\0", &hlsl) {
            Err(msg) => {
                println!("Cannot compile pixelshader\n{}\n{}", msg, split_source(&hlsl));
                return None
            },
            Ok(blob) => {
                blob
            }
        };
        
        let mut vs = None;
        unsafe {d3d11_cx.device.CreateVertexShader(
            std::slice::from_raw_parts(vs_blob.GetBufferPointer() as *const u8, vs_blob.GetBufferSize() as usize),
            None,
            Some(&mut vs)
        ).unwrap()};
        
        let mut ps = None;
        unsafe {d3d11_cx.device.CreatePixelShader(
            std::slice::from_raw_parts(ps_blob.GetBufferPointer() as *const u8, ps_blob.GetBufferSize() as usize),
            None,
            Some(&mut ps)
        ).unwrap()};
        
        let mut layout_desc = Vec::new();
        let mut strings = Vec::new();
        
        for (index, geom) in mapping.geometries.inputs.iter().enumerate() {
            
            strings.push(format!("GEOM{}\0", generate_hlsl::index_to_char(index))); //std::char::from_u32(index as u32 + 65).unwrap())).unwrap());
            
            layout_desc.push(D3D11_INPUT_ELEMENT_DESC {
                SemanticName: PCSTR(strings.last().unwrap().as_ptr()),
                SemanticIndex: 0,
                Format: slots_to_dxgi_format(geom.slots),
                InputSlot: 0,
                AlignedByteOffset: (geom.offset * 4) as u32,
                InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                InstanceDataStepRate: 0
            })
        }
        
        let mut index = 0;
        for inst in &mapping.instances.inputs {
            if inst.slots == 16 {
                for i in 0..4 {
                    strings.push(format!("INST{}\0", generate_hlsl::index_to_char(index))); //std::char::from_u32(index as u32 + 65).unwrap())).unwrap());
                    layout_desc.push(D3D11_INPUT_ELEMENT_DESC {
                        SemanticName: PCSTR(strings.last().unwrap().as_ptr()),
                        SemanticIndex: 0,
                        Format: slots_to_dxgi_format(4),
                        InputSlot: 1,
                        AlignedByteOffset: (inst.offset * 4 + i * 16) as u32,
                        InputSlotClass: D3D11_INPUT_PER_INSTANCE_DATA,
                        InstanceDataStepRate: 1
                    });
                    index += 1;
                }
            }
            else if inst.slots == 9 {
                for i in 0..3 {
                    strings.push(format!("INST{}\0", generate_hlsl::index_to_char(index))); //std::char::from_u32(index as u32 + 65).unwrap())).unwrap());
                    layout_desc.push(D3D11_INPUT_ELEMENT_DESC {
                        SemanticName: PCSTR(strings.last().unwrap().as_ptr()),
                        SemanticIndex: 0,
                        Format: slots_to_dxgi_format(3),
                        InputSlot: 1,
                        AlignedByteOffset: (inst.offset * 4 + i * 9) as u32,
                        InputSlotClass: D3D11_INPUT_PER_INSTANCE_DATA,
                        InstanceDataStepRate: 1
                    });
                    index += 1;
                }
            }
            else {
                strings.push(format!("INST{}\0", generate_hlsl::index_to_char(index))); //std::char::from_u32(index as u32 + 65).unwrap())).unwrap());
                layout_desc.push(D3D11_INPUT_ELEMENT_DESC {
                    SemanticName: PCSTR(strings.last().unwrap().as_ptr()),
                    SemanticIndex: 0,
                    Format: slots_to_dxgi_format(inst.slots),
                    InputSlot: 1,
                    AlignedByteOffset: (inst.offset * 4) as u32,
                    InputSlotClass: D3D11_INPUT_PER_INSTANCE_DATA,
                    InstanceDataStepRate: 1
                });
                index += 1;
            }
        }
        
        let mut input_layout = None;
        unsafe {
            d3d11_cx.device.CreateInputLayout(
                &layout_desc,
                std::slice::from_raw_parts(vs_blob.GetBufferPointer() as *const u8, vs_blob.GetBufferSize() as usize),
                Some(&mut input_layout)
            ).unwrap()
        };
        
        let mut live_uniforms = D3d11Buffer::default();
        live_uniforms.update_with_f32_constant_data(d3d11_cx, mapping.live_uniforms_buf.as_ref());
        
        let mut const_table_uniforms = D3d11Buffer::default();
        const_table_uniforms.update_with_f32_constant_data(d3d11_cx, mapping.const_table.table.as_ref());
        
        Some(Self {
            hlsl,
            const_table_uniforms,
            live_uniforms,
            pixel_shader: ps.unwrap(),
            vertex_shader: vs.unwrap(),
            pixel_shader_blob: ps_blob,
            vertex_shader_blob: vs_blob,
            input_layout: input_layout.unwrap()
        })
    }
}
