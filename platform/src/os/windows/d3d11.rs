use crate::{
    cx::Cx,
    draw_list::DrawListId,
    draw_pass::{DrawPassClearColor, DrawPassClearDepth, DrawPassId},
    draw_shader::{
        CxDrawShader, CxDrawShaderCode, CxDrawShaderMapping, DrawShaderAttrFormat, DrawShaderId,
        UniformBufferBindings,
    },
    draw_vars::DrawVars,
    event::WindowGeom,
    geometry::Geometry,
    makepad_math::*,
    makepad_script::shader::*,
    makepad_script::shader_backend::*,
    makepad_script::*,
    os::{
        windows::win32_app::{FALSE, TRUE},
        windows::win32_window::Win32Window,
    },
    script::vm::*,
    texture::Texture,
    texture::{CxTexture, TextureFormat, TextureId, TexturePixel},
    window::WindowId,
    windows::{
        core::{
            //ComInterface,
            Interface,
            PCSTR,
        },
        Win32::{
            Foundation::{HANDLE, HMODULE, S_FALSE},
            Graphics::{
                Direct3D::{
                    Fxc::D3DCompile,
                    ID3DBlob, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
                    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0,
                },
                Direct3D11::{
                    D3D11CreateDevice, ID3D11BlendState, ID3D11Buffer, ID3D11DepthStencilState,
                    ID3D11DepthStencilView, ID3D11Device, ID3D11DeviceContext, ID3D11InputLayout,
                    ID3D11PixelShader, ID3D11Query, ID3D11RasterizerState, ID3D11RenderTargetView,
                    ID3D11Resource, ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11VertexShader,
                    D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_DEPTH_STENCIL, D3D11_BIND_FLAG,
                    D3D11_BIND_INDEX_BUFFER, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
                    D3D11_BIND_VERTEX_BUFFER, D3D11_BLEND_DESC, D3D11_BLEND_INV_SRC_ALPHA,
                    D3D11_BLEND_ONE, D3D11_BLEND_OP_ADD, D3D11_BUFFER_DESC, D3D11_CLEAR_DEPTH,
                    D3D11_CLEAR_STENCIL, D3D11_COLOR_WRITE_ENABLE_ALL, D3D11_COMPARISON_ALWAYS,
                    D3D11_COMPARISON_LESS_EQUAL, D3D11_CPU_ACCESS_WRITE, D3D11_CREATE_DEVICE_FLAG,
                    D3D11_CULL_NONE, D3D11_DEPTH_STENCILOP_DESC, D3D11_DEPTH_STENCIL_DESC,
                    D3D11_DEPTH_STENCIL_VIEW_DESC, D3D11_DEPTH_WRITE_MASK_ALL,
                    D3D11_DSV_DIMENSION_TEXTURE2D, D3D11_FILL_SOLID, D3D11_INPUT_ELEMENT_DESC,
                    D3D11_INPUT_PER_INSTANCE_DATA, D3D11_INPUT_PER_VERTEX_DATA,
                    D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_WRITE_DISCARD, D3D11_QUERY_DESC,
                    D3D11_QUERY_EVENT, D3D11_RASTERIZER_DESC, D3D11_RENDER_TARGET_BLEND_DESC,
                    D3D11_RESOURCE_MISC_FLAG, D3D11_RESOURCE_MISC_TEXTURECUBE, D3D11_SDK_VERSION, D3D11_STENCIL_OP_REPLACE,
                    D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
                    D3D11_USAGE_DYNAMIC, D3D11_VIEWPORT,
                },
                Dxgi::{
                    Common::{
                        DXGI_ALPHA_MODE_IGNORE,
                        DXGI_FORMAT,
                        DXGI_FORMAT_B8G8R8A8_UNORM,
                        //DXGI_FORMAT_D32_FLOAT_S8X 24_UINT,
                        DXGI_FORMAT_D32_FLOAT,
                        DXGI_FORMAT_R16_FLOAT,
                        DXGI_FORMAT_R32G32B32A32_FLOAT,
                        DXGI_FORMAT_R32G32B32A32_SINT,
                        DXGI_FORMAT_R32G32B32A32_UINT,
                        DXGI_FORMAT_R32G32B32_FLOAT,
                        DXGI_FORMAT_R32G32B32_SINT,
                        DXGI_FORMAT_R32G32B32_UINT,
                        DXGI_FORMAT_R32G32_FLOAT,
                        DXGI_FORMAT_R32G32_SINT,
                        DXGI_FORMAT_R32G32_UINT,
                        DXGI_FORMAT_R32_FLOAT,
                        DXGI_FORMAT_R32_SINT,
                        DXGI_FORMAT_R32_UINT,
                        DXGI_FORMAT_R8G8_UNORM,
                        DXGI_FORMAT_R8_UNORM,
                        DXGI_SAMPLE_DESC,
                    },
                    CreateDXGIFactory2, IDXGIFactory2, IDXGIResource, IDXGISwapChain1,
                    DXGI_CREATE_FACTORY_FLAGS, DXGI_PRESENT, DXGI_RGBA, DXGI_SCALING_NONE,
                    DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_DISCARD,
                    DXGI_USAGE_RENDER_TARGET_OUTPUT,
                },
            },
        },
    },
};

impl Cx {
    fn render_view(
        &mut self,
        pass_id: DrawPassId,
        draw_list_id: DrawListId,
        zbias: &mut f32,
        zbias_step: f32,
        d3d11_cx: &D3d11Cx,
    ) {
        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
        let draw_items_len = self.draw_lists[draw_list_id].draw_items.len();

        {
            let draw_list = &mut self.draw_lists[draw_list_id];
            draw_list
                .os
                .draw_list_uniforms
                .update_with_f32_constant_data(d3d11_cx, draw_list.draw_list_uniforms.as_slice());
        }

        for draw_item_id in 0..draw_items_len {
            if let Some(sub_list_id) = self.draw_lists[draw_list_id].draw_items[draw_item_id]
                .kind
                .sub_list()
            {
                self.render_view(pass_id, sub_list_id, zbias, zbias_step, d3d11_cx);
            } else {
                let draw_list = &mut self.draw_lists[draw_list_id];
                let draw_item = &mut draw_list.draw_items[draw_item_id];
                let draw_call = if let Some(draw_call) = draw_item.kind.draw_call_mut() {
                    draw_call
                } else {
                    continue;
                };
                let sh = &self.draw_shaders[draw_call.draw_shader_id.index];
                if sh.os_shader_id.is_none() {
                    // shader didnt compile somehow
                    continue;
                }
                if sh.mapping.uses_time {
                    self.demo_time_repaint = true;
                }
                let shp = &self.draw_shaders.os_shaders[sh.os_shader_id.unwrap()];

                if draw_call.instance_dirty {
                    draw_call.instance_dirty = false;
                    if draw_item.instances.as_ref().unwrap().len() == 0 {
                        continue;
                    }
                    // update the instance buffer data
                    draw_item.os.inst_vbuf.update_with_f32_vertex_data(
                        d3d11_cx,
                        draw_item.instances.as_ref().unwrap(),
                    );
                }

                // update the zbias uniform if we have it.
                draw_call.draw_call_uniforms.set_zbias(*zbias);
                *zbias += zbias_step;

                if draw_call.uniforms_dirty {
                    draw_call.uniforms_dirty = false;
                    draw_item
                        .os
                        .draw_call_uniforms
                        .update_with_f32_constant_data(
                            d3d11_cx,
                            draw_call.draw_call_uniforms.as_slice(),
                        );
                }
                if draw_call.dyn_uniforms.len() != 0 {
                    draw_item
                        .os
                        .user_uniforms
                        .update_with_f32_constant_data(d3d11_cx, &mut draw_call.dyn_uniforms);
                }

                let instances = (draw_item.instances.as_ref().unwrap().len()
                    / sh.mapping.instances.total_slots) as u64;

                if instances == 0 {
                    continue;
                }

                let geometry_id = if let Some(geometry_id) = draw_call.geometry_id {
                    geometry_id
                } else {
                    continue;
                };

                let geometry = &mut self.geometries[geometry_id];

                if geometry.dirty {
                    geometry
                        .os
                        .geom_ibuf
                        .update_with_u32_index_data(d3d11_cx, &geometry.indices);
                    geometry
                        .os
                        .geom_vbuf
                        .update_with_f32_vertex_data(d3d11_cx, &geometry.vertices);
                    geometry.dirty = false;
                }

                unsafe {
                    d3d11_cx.context.VSSetShader(&shp.vertex_shader, None);
                    d3d11_cx.context.PSSetShader(&shp.pixel_shader, None);
                    d3d11_cx
                        .context
                        .IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
                    d3d11_cx.context.IASetInputLayout(&shp.input_layout);

                    let geom_ibuf = geometry.os.geom_ibuf.buffer.as_ref().unwrap();
                    d3d11_cx
                        .context
                        .IASetIndexBuffer(geom_ibuf, DXGI_FORMAT_R32_UINT, 0);

                    let geom_slots = sh.mapping.geometries.total_slots;
                    let inst_slots = sh.mapping.instances.total_slots;
                    let strides = [(geom_slots * 4) as u32, (inst_slots * 4) as u32];
                    let offsets = [0u32, 0u32];
                    let buffers = [
                        geometry.os.geom_vbuf.buffer.clone(),
                        draw_item.os.inst_vbuf.buffer.clone(),
                    ];
                    d3d11_cx.context.IASetVertexBuffers(
                        0,
                        2,
                        Some(buffers.as_ptr()),
                        Some(strides.as_ptr()),
                        Some(offsets.as_ptr()),
                    );

                    fn buffer_slot(d3d11_cx: &D3d11Cx, index: u32, buffer: &Option<ID3D11Buffer>) {
                        unsafe {
                            if let Some(buffer) = buffer.clone() {
                                let buffers = [Some(buffer)];
                                d3d11_cx.context.VSSetConstantBuffers(index, Some(&buffers));
                                d3d11_cx.context.PSSetConstantBuffers(index, Some(&buffers));
                            } else {
                                d3d11_cx.context.VSSetConstantBuffers(index, None);
                                d3d11_cx.context.PSSetConstantBuffers(index, None);
                            }
                        }
                    }

                    fn buffer_slot_opt(
                        d3d11_cx: &D3d11Cx,
                        index: Option<u32>,
                        buffer: &Option<ID3D11Buffer>,
                    ) {
                        if let Some(idx) = index {
                            buffer_slot(d3d11_cx, idx, buffer);
                        }
                    }

                    buffer_slot(d3d11_cx, 0, &shp.live_uniforms.buffer);
                    buffer_slot(d3d11_cx, 1, &shp.const_table_uniforms.buffer);
                    buffer_slot_opt(
                        d3d11_cx,
                        shp.dyn_uniform_buffer_id,
                        &draw_item.os.user_uniforms.buffer,
                    );
                    buffer_slot_opt(
                        d3d11_cx,
                        shp.draw_call_uniform_buffer_id,
                        &draw_item.os.draw_call_uniforms.buffer,
                    );
                    buffer_slot_opt(
                        d3d11_cx,
                        shp.pass_uniform_buffer_id,
                        &self.passes[pass_id].os.pass_uniforms.buffer,
                    );
                    buffer_slot_opt(
                        d3d11_cx,
                        shp.draw_list_uniform_buffer_id,
                        &draw_list.os.draw_list_uniforms.buffer,
                    );
                    buffer_slot_opt(
                        d3d11_cx,
                        shp.scope_uniform_buffer_id,
                        &shp.scope_uniforms.buffer,
                    );
                }

                for i in 0..sh.mapping.textures.len() {
                    let texture_id = if let Some(texture) = &draw_call.texture_slots[i] {
                        texture.texture_id()
                    } else {
                        unsafe {
                            d3d11_cx.context.PSSetShaderResources(i as u32, None);
                            d3d11_cx.context.VSSetShaderResources(i as u32, None);
                        }
                        continue;
                    };

                    let cxtexture = &mut self.textures[texture_id];

                    if cxtexture.format.is_shared() {
                        cxtexture.update_shared_texture(&d3d11_cx.device);
                    } else if cxtexture.format.is_vec() {
                        cxtexture.update_vec_texture(d3d11_cx);
                    }
                    unsafe {
                        if let Some(sr) = &cxtexture.os.shader_resource_view {
                            d3d11_cx
                                .context
                                .PSSetShaderResources(i as u32, Some(&[Some(sr.clone())]));
                            d3d11_cx
                                .context
                                .VSSetShaderResources(i as u32, Some(&[Some(sr.clone())]));
                        } else {
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
                        0,
                    )
                };
            }
        }
    }

    pub fn get_shared_handle(&self, _texture: &Texture) -> HANDLE {
        self.textures[_texture.texture_id()].os.shared_handle
    }

    pub fn setup_pass_render_targets(
        &mut self,
        pass_id: DrawPassId,
        first_target: &Option<ID3D11RenderTargetView>,
        d3d11_cx: &D3d11Cx,
    ) {
        let dpi_factor = self.passes[pass_id].dpi_factor.unwrap();

        let pass_rect = self.get_pass_rect(pass_id, dpi_factor).unwrap();
        self.passes[pass_id].set_ortho_matrix(pass_rect.pos, pass_rect.size);
        self.passes[pass_id].paint_dirty = false;

        self.passes[pass_id].set_dpi_factor(dpi_factor);

        let viewport = D3D11_VIEWPORT {
            Width: (pass_rect.size.x * dpi_factor) as f32,
            Height: (pass_rect.size.y * dpi_factor) as f32,
            MinDepth: 0.,
            MaxDepth: 1.,
            TopLeftX: 0.0,
            TopLeftY: 0.0,
        };
        unsafe {
            d3d11_cx.context.RSSetViewports(Some(&[viewport]));
        }
        if viewport.Width < 1.0 || viewport.Height < 1.0 {
            return;
        }
        // set up the color texture array
        let mut color_textures = Vec::<Option<ID3D11RenderTargetView>>::new();

        if let Some(render_target) = first_target {
            color_textures.push(Some(render_target.clone()));
            let color = self.passes[pass_id].clear_color;
            let color = [color.x, color.y, color.z, color.w];
            unsafe {
                d3d11_cx
                    .context
                    .ClearRenderTargetView(first_target.as_ref().unwrap(), &color)
            }
        } else {
            for color_texture in self.passes[pass_id].color_textures.iter() {
                let cxtexture = &mut self.textures[color_texture.texture.texture_id()];
                let size = pass_rect.size * dpi_factor;
                cxtexture.update_render_target(d3d11_cx, size.x as usize, size.y as usize);
                let is_initial = cxtexture.take_initial();
                let render_target = cxtexture.os.render_target_view.clone();
                color_textures.push(Some(render_target.clone().unwrap()));
                // possibly clear it
                match color_texture.clear_color {
                    DrawPassClearColor::InitWith(color) => {
                        if is_initial {
                            let color = [color.x, color.y, color.z, color.w];
                            unsafe {
                                d3d11_cx
                                    .context
                                    .ClearRenderTargetView(render_target.as_ref().unwrap(), &color)
                            }
                        }
                    }
                    DrawPassClearColor::ClearWith(color) => {
                        let color = [color.x, color.y, color.z, color.w];
                        unsafe {
                            d3d11_cx
                                .context
                                .ClearRenderTargetView(render_target.as_ref().unwrap(), &color)
                        }
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
                DrawPassClearDepth::InitWith(depth_clear) => {
                    if is_initial {
                        unsafe {
                            d3d11_cx.context.ClearDepthStencilView(
                                cxtexture.os.depth_stencil_view.as_ref().unwrap(),
                                D3D11_CLEAR_DEPTH.0 as u32 | D3D11_CLEAR_STENCIL.0 as u32,
                                depth_clear,
                                0,
                            )
                        }
                    }
                }
                DrawPassClearDepth::ClearWith(depth_clear) => unsafe {
                    d3d11_cx.context.ClearDepthStencilView(
                        cxtexture.os.depth_stencil_view.as_ref().unwrap(),
                        D3D11_CLEAR_DEPTH.0 as u32 | D3D11_CLEAR_STENCIL.0 as u32,
                        depth_clear,
                        0,
                    )
                },
            }
            unsafe {
                d3d11_cx.context.OMSetRenderTargets(
                    Some(&color_textures),
                    None, //cxtexture.os.depth_stencil_view.as_ref().unwrap()
                )
            }
        } else {
            unsafe {
                d3d11_cx
                    .context
                    .OMSetRenderTargets(Some(&color_textures), None)
            }
        }

        // create depth, blend and raster states
        self.passes[pass_id].os.set_states(d3d11_cx);

        let cxpass = &mut self.passes[pass_id];

        cxpass
            .os
            .pass_uniforms
            .update_with_f32_constant_data(&d3d11_cx, cxpass.pass_uniforms.as_slice());
    }

    pub fn draw_pass_to_window(
        &mut self,
        pass_id: DrawPassId,
        vsync: bool,
        d3d11_window: &mut D3d11Window,
        d3d11_cx: &D3d11Cx,
    ) {
        // let time1 = Cx::profile_time_ns();
        let draw_list_id = self.passes[pass_id].main_draw_list_id.unwrap();

        self.setup_pass_render_targets(pass_id, &d3d11_window.render_target_view, d3d11_cx);

        let mut zbias = 0.0;
        let zbias_step = self.passes[pass_id].zbias_step;

        self.render_view(pass_id, draw_list_id, &mut zbias, zbias_step, d3d11_cx);
        d3d11_window.present(vsync);
        if d3d11_window.first_draw {
            d3d11_window.win32_window.show();
            d3d11_window.first_draw = false;
        }
        //println!("{}", (Cx::profile_time_ns() - time1)as f64 / 1000.0);
    }

    pub fn draw_pass_to_texture(
        &mut self,
        pass_id: DrawPassId,
        d3d11_cx: &D3d11Cx,
        texture_id: Option<TextureId>,
    ) {
        // let time1 = Cx::profile_time_ns();
        let draw_list_id = self.passes[pass_id].main_draw_list_id.unwrap();

        if let Some(texture_id) = texture_id {
            let render_target_view = self.textures[texture_id].os.render_target_view.clone();
            self.setup_pass_render_targets(pass_id, &render_target_view, d3d11_cx);
        } else {
            self.setup_pass_render_targets(pass_id, &None, d3d11_cx);
        }

        let mut zbias = 0.0;
        let zbias_step = self.passes[pass_id].zbias_step;
        self.render_view(pass_id, draw_list_id, &mut zbias, zbias_step, &d3d11_cx);
    }

    pub(crate) fn hlsl_compile_shaders(&mut self, d3d11_cx: &D3d11Cx) {
        for draw_shader_id in self
            .draw_shaders
            .compile_set
            .iter()
            .cloned()
            .collect::<Vec<_>>()
        {
            let cx_shader = &self.draw_shaders.shaders[draw_shader_id];

            let hlsl = match &cx_shader.mapping.code {
                CxDrawShaderCode::Combined { code } => code.clone(),
                CxDrawShaderCode::Separate { .. } => {
                    crate::error!("D3D11 does not support separate vertex/fragment sources");
                    continue;
                }
            };

            if cx_shader.mapping.flags.debug {
                crate::log!("{}", hlsl);
            }

            // Get the uniform buffer bindings from the mapping
            let bindings = cx_shader.mapping.uniform_buffer_bindings.clone();

            // Check if we already have an os_shader with the same source
            let mut found_os_shader_id = None;
            for (index, ds) in self.draw_shaders.os_shaders.iter().enumerate() {
                if ds.hlsl == hlsl {
                    found_os_shader_id = Some(index);
                    break;
                }
            }

            let cx_shader = &mut self.draw_shaders.shaders[draw_shader_id];
            if let Some(os_shader_id) = found_os_shader_id {
                cx_shader.os_shader_id = Some(os_shader_id);
            } else {
                if let Some(shp) =
                    CxOsDrawShader::new(d3d11_cx, hlsl, &cx_shader.mapping, &bindings)
                {
                    cx_shader.os_shader_id = Some(self.draw_shaders.os_shaders.len());
                    self.draw_shaders.os_shaders.push(shp);
                }
            }
        }
        self.draw_shaders.compile_set.clear();
    }

    pub fn share_texture_for_presentable_image(&mut self, texture: &Texture) -> u64 {
        let cxtexture = &mut self.textures[texture.texture_id()];
        cxtexture.update_shared_texture(self.os.d3d11_device.as_ref().unwrap());
        cxtexture.os.shared_handle.0 as u64
    }
}

fn texture_pixel_to_dx11_pixel(pix: &TexturePixel) -> DXGI_FORMAT {
    match pix {
        TexturePixel::BGRAu8 => DXGI_FORMAT_B8G8R8A8_UNORM,
        TexturePixel::RGBAf16 => DXGI_FORMAT_R16_FLOAT,
        TexturePixel::RGBAf32 => DXGI_FORMAT_R32G32B32A32_FLOAT,
        TexturePixel::Ru8 => DXGI_FORMAT_R8_UNORM,
        TexturePixel::RGu8 => DXGI_FORMAT_R8G8_UNORM,
        TexturePixel::Rf32 => DXGI_FORMAT_R32_FLOAT,
        TexturePixel::D32 => DXGI_FORMAT_D32_FLOAT,
    }
}

pub struct D3d11Window {
    pub window_id: WindowId,
    pub is_in_resize: bool,
    pub window_geom: WindowGeom,
    pub win32_window: Box<Win32Window>,
    pub render_target_view: Option<ID3D11RenderTargetView>,
    pub swap_texture: Option<ID3D11Texture2D>,
    pub alloc_size: Vec2d,
    pub first_draw: bool,
    pub swap_chain: IDXGISwapChain1,
}

impl D3d11Window {
    pub fn new(
        window_id: WindowId,
        d3d11_cx: &D3d11Cx,
        inner_size: Vec2d,
        position: Option<Vec2d>,
        title: &str,
        is_fullscreen: bool,
    ) -> D3d11Window {
        // create window, and then initialize it; this is needed because
        // GWLP_USERDATA needs to reference a stable and existing window
        let mut win32_window =
            Box::new(Win32Window::new(window_id, title, position, is_fullscreen));
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
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Scaling: DXGI_SCALING_NONE,
            Stereo: FALSE,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
        };

        unsafe {
            let swap_chain = d3d11_cx
                .factory
                .CreateSwapChainForHwnd(&d3d11_cx.device, win32_window.hwnd, &sc_desc, None, None)
                .unwrap();

            let swap_texture = swap_chain.GetBuffer(0).unwrap();
            let mut render_target_view = None;
            d3d11_cx
                .device
                .CreateRenderTargetView(&swap_texture, None, Some(&mut render_target_view))
                .unwrap();
            swap_chain
                .SetBackgroundColor(&mut DXGI_RGBA {
                    r: 0.3,
                    g: 0.3,
                    b: 0.3,
                    a: 1.0,
                })
                .unwrap();
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
        self.alloc_size = Vec2d::default();
    }

    pub fn resize_buffers(&mut self, d3d11_cx: &D3d11Cx) {
        if self.alloc_size == self.window_geom.inner_size {
            return;
        }
        self.alloc_size = self.window_geom.inner_size;
        self.swap_texture = None;
        self.render_target_view = None;

        unsafe {
            let wg = &self.window_geom;
            self.swap_chain
                .ResizeBuffers(
                    2,
                    (wg.inner_size.x * wg.dpi_factor) as u32,
                    (wg.inner_size.y * wg.dpi_factor) as u32,
                    DXGI_FORMAT_B8G8R8A8_UNORM,
                    DXGI_SWAP_CHAIN_FLAG(0),
                )
                .unwrap();

            let swap_texture = self.swap_chain.GetBuffer(0).unwrap();
            let mut render_target_view = None;
            d3d11_cx
                .device
                .CreateRenderTargetView(&swap_texture, None, Some(&mut render_target_view))
                .unwrap();

            self.swap_texture = Some(swap_texture);
            self.render_target_view = render_target_view;
        }
    }

    pub fn present(&mut self, vsync: bool) {
        unsafe {
            self.swap_chain
                .Present(if vsync { 1 } else { 0 }, DXGI_PRESENT(0))
                .unwrap()
        };
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
            let factory: IDXGIFactory2 =
                CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).unwrap();
            let adapter = factory.EnumAdapters(0).unwrap();
            let mut device: Option<ID3D11Device> = None;
            let mut context: Option<ID3D11DeviceContext> = None;
            let mut query: Option<ID3D11Query> = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE(std::ptr::null_mut()),
                D3D11_CREATE_DEVICE_FLAG(0),
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .unwrap();

            let device = device.unwrap();
            let context = context.unwrap();

            device
                .CreateQuery(
                    &D3D11_QUERY_DESC {
                        Query: D3D11_QUERY_EVENT,
                        MiscFlags: 0,
                    },
                    Some(&mut query),
                )
                .unwrap();

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
        let hresult = unsafe {
            (Interface::vtable(&self.context).GetData)(
                Interface::as_raw(&self.context),
                Interface::as_raw(&self.query),
                std::ptr::null_mut(),
                0,
                0,
            )
        };
        hresult != S_FALSE
    }
}

#[derive(Clone, Default)]
pub struct CxOsDrawList {
    pub draw_list_uniforms: D3d11Buffer,
}

#[derive(Default, Clone)]
pub struct CxOsDrawCall {
    pub draw_call_uniforms: D3d11Buffer,
    pub user_uniforms: D3d11Buffer,
    pub inst_vbuf: D3d11Buffer,
}

#[derive(Default, Clone)]
pub struct D3d11Buffer {
    pub last_size: usize,
    pub buffer: Option<ID3D11Buffer>,
}

impl D3d11Buffer {
    fn create_buffer_or_update(
        &mut self,
        d3d11_cx: &D3d11Cx,
        buffer_desc: &D3D11_BUFFER_DESC,
        sub_data: &D3D11_SUBRESOURCE_DATA,
        len_slots: usize,
        data: *const std::ffi::c_void,
    ) {
        if self.last_size == len_slots {
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            let p_mapped: *mut _ = &mut mapped;
            unsafe {
                d3d11_cx
                    .context
                    .Map(
                        self.buffer.as_ref().unwrap(),
                        0,
                        D3D11_MAP_WRITE_DISCARD,
                        0,
                        Some(p_mapped),
                    )
                    .unwrap();

                std::ptr::copy_nonoverlapping(data, mapped.pData, len_slots * 4);
                d3d11_cx.context.Unmap(self.buffer.as_ref().unwrap(), 0);
            }
        } else {
            self.last_size = len_slots;
            unsafe {
                d3d11_cx
                    .device
                    .CreateBuffer(buffer_desc, Some(sub_data), Some(&mut self.buffer))
                    .unwrap()
            }
        }
    }

    pub fn update_with_data(
        &mut self,
        d3d11_cx: &D3d11Cx,
        bind_flags: D3D11_BIND_FLAG,
        len_slots: usize,
        data: *const std::ffi::c_void,
    ) {
        let buffer_desc = D3D11_BUFFER_DESC {
            Usage: D3D11_USAGE_DYNAMIC,
            ByteWidth: (len_slots * 4) as u32,
            BindFlags: bind_flags.0 as u32,
            CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
            MiscFlags: 0,
            StructureByteStride: 0,
        };

        let sub_data = D3D11_SUBRESOURCE_DATA {
            pSysMem: data,
            SysMemPitch: 0,
            SysMemSlicePitch: 0,
        };
        self.create_buffer_or_update(d3d11_cx, &buffer_desc, &sub_data, len_slots, data);
    }

    pub fn update_with_u32_index_data(&mut self, d3d11_cx: &D3d11Cx, data: &[u32]) {
        self.update_with_data(
            d3d11_cx,
            D3D11_BIND_INDEX_BUFFER,
            data.len(),
            data.as_ptr() as *const _,
        );
    }

    pub fn update_with_f32_vertex_data(&mut self, d3d11_cx: &D3d11Cx, data: &[f32]) {
        self.update_with_data(
            d3d11_cx,
            D3D11_BIND_VERTEX_BUFFER,
            data.len(),
            data.as_ptr() as *const _,
        );
    }

    pub fn update_with_f32_constant_data(&mut self, d3d11_cx: &D3d11Cx, data: &[f32]) {
        if data.len() == 0 {
            return;
        }
        if (data.len() & 3) != 0 {
            // we have to align the data at the end
            let mut new_data = data.to_vec();
            let steps = 4 - (data.len() & 3);
            for _ in 0..steps {
                new_data.push(0.0);
            }
            return self.update_with_f32_constant_data(d3d11_cx, &new_data);
        }
        let sub_data = D3D11_SUBRESOURCE_DATA {
            pSysMem: data.as_ptr() as *const _,
            SysMemPitch: 0,
            SysMemSlicePitch: 0,
        };
        let len_slots = data.len();

        let buffer_desc = D3D11_BUFFER_DESC {
            Usage: D3D11_USAGE_DYNAMIC,
            ByteWidth: (len_slots * 4) as u32,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
            MiscFlags: 0,
            StructureByteStride: 0,
        };
        let data = unsafe {
            core::slice::from_raw_parts(
                data.as_ptr() as *const u8,
                std::mem::size_of::<f32>() * data.len(),
            )
            .as_ptr() as *const _
        };
        self.create_buffer_or_update(d3d11_cx, &buffer_desc, &sub_data, len_slots, data);
    }
}

#[derive(Default)]
pub struct CxOsTexture {
    texture: Option<ID3D11Texture2D>,
    pub shared_handle: HANDLE,
    shader_resource_view: Option<ID3D11ShaderResourceView>,
    render_target_view: Option<ID3D11RenderTargetView>,
    depth_stencil_view: Option<ID3D11DepthStencilView>,
}

impl CxTexture {
    pub fn update_vec_texture(&mut self, d3d11_cx: &D3d11Cx) {
        // TODO maybe we can update the data instead of making a new texture?
        if self.alloc_vec() {}
        if !self.take_updated().is_empty() {
            if let TextureFormat::VecCubeBGRAu8_32 {
                width,
                height,
                data,
                ..
            } = &self.format
            {
                let texture_desc = D3D11_TEXTURE2D_DESC {
                    Width: *width as u32,
                    Height: *height as u32,
                    MipLevels: 1,
                    ArraySize: 6,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                    CPUAccessFlags: 0,
                    MiscFlags: D3D11_RESOURCE_MISC_TEXTURECUBE.0 as u32,
                };

                let face_pixels = width.saturating_mul(*height);
                let mut sub_data = Vec::with_capacity(6);
                for face in 0..6usize {
                    let p_sys_mem = if let Some(data) = data.as_ref() {
                        if data.len() >= face_pixels.saturating_mul(6) {
                            unsafe { data.as_ptr().add(face.saturating_mul(face_pixels)) as *const _ }
                        } else {
                            std::ptr::null()
                        }
                    } else {
                        std::ptr::null()
                    };
                    sub_data.push(D3D11_SUBRESOURCE_DATA {
                        pSysMem: p_sys_mem,
                        SysMemPitch: (width.saturating_mul(4)) as u32,
                        SysMemSlicePitch: (width.saturating_mul(*height).saturating_mul(4)) as u32,
                    });
                }

                let mut texture = None;
                unsafe {
                    d3d11_cx
                        .device
                        .CreateTexture2D(&texture_desc, Some(sub_data.as_ptr()), Some(&mut texture))
                        .unwrap()
                };
                let resource: ID3D11Resource = texture.clone().unwrap().cast().unwrap();
                let mut shader_resource_view = None;
                unsafe {
                    d3d11_cx
                        .device
                        .CreateShaderResourceView(&resource, None, Some(&mut shader_resource_view))
                        .unwrap()
                };
                self.os.texture = texture;
                self.os.shader_resource_view = shader_resource_view;
                return;
            }

            fn get_descs(
                format: DXGI_FORMAT,
                width: usize,
                height: usize,
                bpp: usize,
                data: *const std::ffi::c_void,
            ) -> (D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC) {
                let sub_data = D3D11_SUBRESOURCE_DATA {
                    pSysMem: data,
                    SysMemPitch: (width * bpp) as u32,
                    SysMemSlicePitch: 0,
                };

                let texture_desc = D3D11_TEXTURE2D_DESC {
                    Width: width as u32,
                    Height: height as u32,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: format,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                    CPUAccessFlags: 0,
                    MiscFlags: 0,
                };
                (sub_data, texture_desc)
            }

            let (sub_data, texture_desc) = match &self.format {
                TextureFormat::VecBGRAu8_32 {
                    width,
                    height,
                    data,
                    ..
                } => get_descs(
                    DXGI_FORMAT_B8G8R8A8_UNORM,
                    *width,
                    *height,
                    4,
                    data.as_ref().unwrap().as_ptr() as *const _,
                ),
                TextureFormat::VecRGBAf32 {
                    width,
                    height,
                    data,
                    ..
                } => get_descs(
                    DXGI_FORMAT_R32G32B32A32_FLOAT,
                    *width,
                    *height,
                    16,
                    data.as_ref().unwrap().as_ptr() as *const _,
                ),
                TextureFormat::VecRu8 {
                    width,
                    height,
                    data,
                    ..
                } => get_descs(
                    DXGI_FORMAT_R8_UNORM,
                    *width,
                    *height,
                    1,
                    data.as_ref().unwrap().as_ptr() as *const _,
                ),
                TextureFormat::VecRGu8 {
                    width,
                    height,
                    data,
                    ..
                } => get_descs(
                    DXGI_FORMAT_R8G8_UNORM,
                    *width,
                    *height,
                    1,
                    data.as_ref().unwrap().as_ptr() as *const _,
                ),
                TextureFormat::VecRf32 {
                    width,
                    height,
                    data,
                    ..
                } => get_descs(
                    DXGI_FORMAT_R32_FLOAT,
                    *width,
                    *height,
                    4,
                    data.as_ref().unwrap().as_ptr() as *const _,
                ),
                _ => panic!(),
            };

            let mut texture = None;
            unsafe {
                d3d11_cx
                    .device
                    .CreateTexture2D(&texture_desc, Some(&sub_data), Some(&mut texture))
                    .unwrap()
            };
            let resource: ID3D11Resource = texture.clone().unwrap().cast().unwrap();
            let mut shader_resource_view = None;
            unsafe {
                d3d11_cx
                    .device
                    .CreateShaderResourceView(&resource, None, Some(&mut shader_resource_view))
                    .unwrap()
            };
            self.os.texture = texture;
            self.os.shader_resource_view = shader_resource_view;
        }
    }

    pub fn update_render_target(&mut self, d3d11_cx: &D3d11Cx, width: usize, height: usize) {
        if self.alloc_render(width, height) {
            let alloc = self.alloc.as_ref().unwrap();
            let misc_flags = D3D11_RESOURCE_MISC_FLAG(0);
            let format = texture_pixel_to_dx11_pixel(&alloc.pixel);

            let texture_desc = D3D11_TEXTURE2D_DESC {
                Width: width as u32,
                Height: height as u32,
                MipLevels: 1,
                ArraySize: 1,
                Format: format,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                CPUAccessFlags: 0,
                MiscFlags: misc_flags.0 as u32,
            };

            let mut texture = None;
            unsafe {
                d3d11_cx
                    .device
                    .CreateTexture2D(&texture_desc, None, Some(&mut texture))
                    .unwrap()
            };
            let resource: ID3D11Resource = texture.clone().unwrap().cast().unwrap();
            let mut shader_resource_view = None;
            unsafe {
                d3d11_cx
                    .device
                    .CreateShaderResourceView(&resource, None, Some(&mut shader_resource_view))
                    .unwrap()
            };
            let mut render_target_view = None;
            unsafe {
                d3d11_cx
                    .device
                    .CreateRenderTargetView(&resource, None, Some(&mut render_target_view))
                    .unwrap()
            };

            self.os.texture = texture;
            self.os.shader_resource_view = shader_resource_view;
            self.os.render_target_view = render_target_view;
        }
    }

    pub fn update_depth_stencil(&mut self, d3d11_cx: &D3d11Cx, width: usize, height: usize) {
        if self.alloc_depth(width, height) {
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
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_DEPTH_STENCIL.0 as u32, // | D3D11_BIND_SHADER_RESOURCE,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };

            let mut texture = None;
            unsafe {
                d3d11_cx
                    .device
                    .CreateTexture2D(&texture_desc, None, Some(&mut texture))
                    .unwrap()
            };
            let resource: ID3D11Resource = texture.clone().unwrap().cast().unwrap();
            //let shader_resource_view = unsafe {d3d11_cx.device.CreateShaderResourceView(&texture, None).unwrap()};

            let dsv_desc = D3D11_DEPTH_STENCIL_VIEW_DESC {
                Format: DXGI_FORMAT_D32_FLOAT,
                ViewDimension: D3D11_DSV_DIMENSION_TEXTURE2D,
                Flags: 0,
                ..Default::default()
            };

            let mut depth_stencil_view = None;
            unsafe {
                d3d11_cx
                    .device
                    .CreateDepthStencilView(
                        &resource,
                        Some(&dsv_desc),
                        Some(&mut depth_stencil_view),
                    )
                    .unwrap()
            };

            self.os.depth_stencil_view = depth_stencil_view;
            self.os.texture = texture;
            self.os.shader_resource_view = None; //Some(shader_resource_view);
        }
    }

    fn update_shared_texture(&mut self, d3d11_device: &ID3D11Device) {
        if self.alloc_shared() {
            let alloc = self.alloc.as_ref().unwrap();

            let texture_desc = D3D11_TEXTURE2D_DESC {
                Width: alloc.width as u32,
                Height: alloc.height as u32,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                CPUAccessFlags: 0,
                MiscFlags: 2, // D3D11_RESOURCE_MISC_SHARED
            };

            let mut texture = None;
            unsafe {
                d3d11_device
                    .CreateTexture2D(&texture_desc, None, Some(&mut texture))
                    .unwrap()
            };
            let resource: ID3D11Resource = texture.clone().unwrap().cast().unwrap();
            let mut shader_resource_view = None;
            unsafe {
                d3d11_device
                    .CreateShaderResourceView(&resource, None, Some(&mut shader_resource_view))
                    .unwrap()
            };

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

    pub fn update_from_shared_handle(&mut self, d3d11_cx: &D3d11Cx, handle: HANDLE) {
        if self.alloc_shared() {
            let mut texture: Option<ID3D11Texture2D> = None;
            if let Ok(()) = unsafe { d3d11_cx.device.OpenSharedResource(handle, &mut texture) } {
                let resource: ID3D11Resource = texture.clone().unwrap().cast().unwrap();
                let mut shader_resource_view = None;
                unsafe {
                    d3d11_cx
                        .device
                        .CreateShaderResourceView(&resource, None, Some(&mut shader_resource_view))
                        .unwrap()
                };
                let mut render_target_view = None;
                unsafe {
                    d3d11_cx
                        .device
                        .CreateRenderTargetView(&resource, None, Some(&mut render_target_view))
                        .unwrap()
                };
                self.os.texture = texture;
                self.os.render_target_view = render_target_view;
                self.os.shader_resource_view = shader_resource_view;
            }
        }
    }
}

impl CxOsPass {
    pub fn set_states(&mut self, d3d11_cx: &D3d11Cx) {
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
            unsafe {
                d3d11_cx
                    .device
                    .CreateBlendState(&blend_desc, Some(&mut self.blend_state))
                    .unwrap()
            }
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
            unsafe {
                d3d11_cx
                    .device
                    .CreateRasterizerState(&raster_desc, Some(&mut self.raster_state))
                    .unwrap()
            }
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
            unsafe {
                d3d11_cx
                    .device
                    .CreateDepthStencilState(&ds_desc, Some(&mut self.depth_stencil_state))
                    .unwrap()
            }
        }

        unsafe {
            d3d11_cx
                .context
                .RSSetState(self.raster_state.as_ref().unwrap());
            let blend_factor = [0., 0., 0., 0.];
            d3d11_cx.context.OMSetBlendState(
                self.blend_state.as_ref().unwrap(),
                Some(&blend_factor),
                0xffffffff,
            );
            d3d11_cx
                .context
                .OMSetDepthStencilState(self.depth_stencil_state.as_ref().unwrap(), 0);
        }
    }
}

#[derive(Default, Clone)]
pub struct CxOsPass {
    pass_uniforms: D3d11Buffer,
    blend_state: Option<ID3D11BlendState>,
    raster_state: Option<ID3D11RasterizerState>,
    depth_stencil_state: Option<ID3D11DepthStencilState>,
}

#[derive(Default, Clone)]
pub struct CxOsGeometry {
    pub geom_vbuf: D3d11Buffer,
    pub geom_ibuf: D3d11Buffer,
}

// Shader compilation for HLSL
impl DrawVars {
    pub(crate) fn compile_shader(&mut self, vm: &mut ScriptVm, _apply: &Apply, value: ScriptValue) {
        // Compile an HLSL shader
        if let Some(io_self) = value.as_object() {
            // Cache 1: Check if this exact object has been compiled before
            {
                let cx = vm.host.cx();
                if let Some(&shader_id) = cx.draw_shaders.cache_object_id_to_shader.get(&io_self) {
                    self.finalize_cached_shader(vm, shader_id);
                    return;
                }
            }

            // Cache 2: Compute function hash and check if we've seen these functions before
            let fnhash = DrawVars::compute_shader_functions_hash(&vm.bx.heap, io_self);
            {
                let cx = vm.host.cx();
                if let Some(&shader_id) = cx.draw_shaders.cache_functions_to_shader.get(&fnhash) {
                    let cx = vm.host.cx_mut();
                    cx.draw_shaders
                        .cache_object_id_to_shader
                        .insert(io_self, shader_id);
                    self.finalize_cached_shader(vm, shader_id);
                    return;
                }
            }

            let mut output = ShaderOutput::default();
            output.backend = ShaderBackend::Hlsl;

            output.pre_collect_rust_instance_io(vm, io_self);
            output.pre_collect_shader_io(vm, io_self);

            if let Some(fnobj) = vm
                .bx
                .heap
                .object_method(io_self, id!(vertex).into(), vm.thread().trap.pass())
                .as_object()
            {
                output.mode = ShaderMode::Vertex;
                ShaderFnCompiler::compile_shader_def(
                    vm,
                    &mut output,
                    NoTrap,
                    id!(vertex),
                    fnobj,
                    ShaderType::IoSelf(io_self),
                    vec![],
                );
            }
            if let Some(fnobj) = vm
                .bx
                .heap
                .object_method(io_self, id!(fragment).into(), vm.thread().trap.pass())
                .as_object()
            {
                output.mode = ShaderMode::Fragment;
                ShaderFnCompiler::compile_shader_def(
                    vm,
                    &mut output,
                    NoTrap,
                    id!(fragment),
                    fnobj,
                    ShaderType::IoSelf(io_self),
                    vec![],
                );
            }

            // Don't proceed if shader compilation had errors
            if output.has_errors {
                return;
            }

            // Assign buffer indices to uniform buffers before generating HLSL code
            // In HLSL, cbuffer registers start from b0
            // b0 = live uniforms, b1 = const table, b2 = draw call, b3 = pass, b4 = draw list, b5 = user
            output.assign_uniform_buffer_indices(&vm.bx.heap, 3);

            let mut out = String::new();
            output.create_struct_defs(vm, &mut out);
            output.hlsl_create_uniform_buffer_cbuffers(vm, &mut out);
            output.hlsl_create_uniform_struct(vm, &mut out);
            output.hlsl_create_scope_uniform_cbuffer(vm, &mut out);
            output.hlsl_create_instance_struct(vm, &mut out);
            output.hlsl_create_varying_struct(vm, &mut out);
            output.hlsl_create_vertex_buffer_struct(vm, &mut out);
            output.hlsl_create_vertex_input_struct(vm, &mut out);
            output.hlsl_create_io_structs(vm, &mut out);
            output.hlsl_create_fragment_output_struct(vm, &mut out);
            output.hlsl_create_texture_samplers(vm, &mut out);
            output.hlsl_create_helpers(vm, &mut out);
            output.create_functions(&mut out);
            output.hlsl_create_vertex_fn(vm, &mut out);
            output.hlsl_create_fragment_fn(vm, &mut out);

            let source = vm.bx.heap.new_object_ref(io_self);

            // Create the shader mapping and allocate CxDrawShader
            let code = CxDrawShaderCode::Combined { code: out };

            // Cache 3: Check if this exact code has been compiled before
            {
                let cx = vm.host.cx();
                if let Some(&shader_id) = cx.draw_shaders.cache_code_to_shader.get(&code) {
                    let cx = vm.host.cx_mut();
                    cx.draw_shaders
                        .cache_object_id_to_shader
                        .insert(io_self, shader_id);
                    cx.draw_shaders
                        .cache_functions_to_shader
                        .insert(fnhash, shader_id);
                    self.finalize_cached_shader(vm, shader_id);
                    return;
                }
            }

            // Extract geometry_id from the vertex buffer object before creating the mapping
            let geometry_id = if let Some(vb_obj) = output.find_vertex_buffer_object(vm, io_self) {
                let buffer_value =
                    vm.bx
                        .heap
                        .value(vb_obj, id!(buffer).into(), vm.thread().trap.pass());
                if let Some(handle) = buffer_value.as_handle() {
                    vm.bx
                        .heap
                        .handle_ref::<Geometry>(handle)
                        .map(|g| g.geometry_id())
                } else {
                    None
                }
            } else {
                None
            };

            let mut mapping = CxDrawShaderMapping::from_shader_output(
                source,
                code.clone(),
                &vm.bx.heap,
                &output,
                geometry_id,
            );

            // Fill the scope uniform buffer from current script values
            mapping.fill_scope_uniforms_buffer(&vm.bx.heap, &vm.thread().trap.pass());

            // Check for debug: true on the shader object
            let debug_value = vm.bx.heap.value(io_self, id!(debug).into(), NoTrap);
            if let Some(true) = debug_value.as_bool() {
                mapping.flags.debug = true;
            }

            // Set dyn_instance_start and dyn_instance_slots based on mapping
            self.dyn_instance_start = self.dyn_instances.len() - mapping.dyn_instances.total_slots;
            self.dyn_instance_slots = mapping.instances.total_slots;

            // Access Cx from the vm host
            let cx = vm.host.cx_mut();

            // Allocate CxDrawShader with os_shader_id set to None
            let index = cx.draw_shaders.shaders.len();
            cx.draw_shaders.shaders.push(CxDrawShader {
                debug_id: LiveId(0),
                os_shader_id: None,
                mapping,
            });

            // Create the shader ID
            let shader_id = DrawShaderId { index };

            // Add to all caches
            cx.draw_shaders
                .cache_object_id_to_shader
                .insert(io_self, shader_id);
            cx.draw_shaders
                .cache_functions_to_shader
                .insert(fnhash, shader_id);
            cx.draw_shaders.cache_code_to_shader.insert(code, shader_id);

            // Add to compile set for later HLSL compilation
            cx.draw_shaders.compile_set.insert(index);

            // Set draw_shader on self
            self.draw_shader_id = Some(shader_id);

            // Use the geometry_id stored on the mapping
            self.geometry_id = geometry_id;
        }
    }
}

#[derive(Clone)]
pub struct CxOsDrawShader {
    pub hlsl: String,
    pub const_table_uniforms: D3d11Buffer,
    pub live_uniforms: D3d11Buffer,
    pub scope_uniforms: D3d11Buffer,
    pub pixel_shader: ID3D11PixelShader,
    pub vertex_shader: ID3D11VertexShader,
    pub pixel_shader_blob: ID3DBlob,
    pub vertex_shader_blob: ID3DBlob,
    pub input_layout: ID3D11InputLayout,
    // Dynamic buffer indices looked up from shader output
    pub draw_call_uniform_buffer_id: Option<u32>,
    pub pass_uniform_buffer_id: Option<u32>,
    pub draw_list_uniform_buffer_id: Option<u32>,
    pub dyn_uniform_buffer_id: Option<u32>,
    pub scope_uniform_buffer_id: Option<u32>,
}

impl CxOsDrawShader {
    fn new(
        d3d11_cx: &D3d11Cx,
        hlsl: String,
        mapping: &CxDrawShaderMapping,
        bindings: &UniformBufferBindings,
    ) -> Option<Self> {
        fn compile_shader(target: &str, entry: &str, shader: &str) -> Result<ID3DBlob, String> {
            const D3DCOMPILE_ENABLE_BACKWARDS_COMPATIBILITY: u32 = 1 << 12;
            unsafe {
                let shader_bytes = shader.as_bytes();
                let mut blob = None;
                let mut errors = None;
                if D3DCompile(
                    shader_bytes.as_ptr() as *const _,
                    shader_bytes.len(),
                    PCSTR("makepad_shader\0".as_ptr()), // sourcename
                    None,                               // defines
                    None,                               // include
                    PCSTR(entry.as_ptr()),              // entry point
                    PCSTR(target.as_ptr()),             // target
                    D3DCOMPILE_ENABLE_BACKWARDS_COMPATIBILITY,
                    0,                                  // flags2
                    &mut blob,
                    Some(&mut errors),
                )
                .is_ok()
                {
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
            return r;
        }

        fn slots_to_dxgi_format(slots: usize, attr_format: DrawShaderAttrFormat) -> DXGI_FORMAT {
            match attr_format {
                DrawShaderAttrFormat::Float => match slots {
                    1 => DXGI_FORMAT_R32_FLOAT,
                    2 => DXGI_FORMAT_R32G32_FLOAT,
                    3 => DXGI_FORMAT_R32G32B32_FLOAT,
                    4 => DXGI_FORMAT_R32G32B32A32_FLOAT,
                    _ => panic!("slots_to_dxgi_format unsupported float slotcount {}", slots),
                },
                DrawShaderAttrFormat::UInt => match slots {
                    1 => DXGI_FORMAT_R32_UINT,
                    2 => DXGI_FORMAT_R32G32_UINT,
                    3 => DXGI_FORMAT_R32G32B32_UINT,
                    4 => DXGI_FORMAT_R32G32B32A32_UINT,
                    _ => panic!("slots_to_dxgi_format unsupported uint slotcount {}", slots),
                },
                DrawShaderAttrFormat::SInt => match slots {
                    1 => DXGI_FORMAT_R32_SINT,
                    2 => DXGI_FORMAT_R32G32_SINT,
                    3 => DXGI_FORMAT_R32G32B32_SINT,
                    4 => DXGI_FORMAT_R32G32B32A32_SINT,
                    _ => panic!("slots_to_dxgi_format unsupported sint slotcount {}", slots),
                },
            }
        }
        fn slot_chunks(slots: usize) -> Vec<usize> {
            match slots {
                0 => Vec::new(),
                // Keep matrix layouts aligned with HLSL matrix input expansion.
                9 => vec![3, 3, 3],
                16 => vec![4, 4, 4, 4],
                _ => {
                    let mut rem = slots;
                    let mut chunks = Vec::new();
                    while rem > 0 {
                        let chunk = rem.min(4);
                        chunks.push(chunk);
                        rem -= chunk;
                    }
                    chunks
                }
            }
        }
        fn index_to_char(index: usize) -> char {
            std::char::from_u32(index as u32 + 65).unwrap_or('?')
        }

        let vs_blob = match compile_shader("vs_5_0\0", "vertex_main\0", &hlsl) {
            Err(msg) => {
                println!(
                    "Cannot compile vertexshader\n{}\n{}",
                    msg,
                    split_source(&hlsl)
                );
                std::process::exit(1);
            }
            Ok(blob) => blob,
        };

        let ps_blob = match compile_shader("ps_5_0\0", "pixel_main\0", &hlsl) {
            Err(msg) => {
                println!(
                    "Cannot compile pixelshader\n{}\n{}",
                    msg,
                    split_source(&hlsl)
                );
                std::process::exit(1);
            }
            Ok(blob) => blob,
        };

        let mut vs = None;
        unsafe {
            d3d11_cx
                .device
                .CreateVertexShader(
                    std::slice::from_raw_parts(
                        vs_blob.GetBufferPointer() as *const u8,
                        vs_blob.GetBufferSize() as usize,
                    ),
                    None,
                    Some(&mut vs),
                )
                .unwrap()
        };

        let mut ps = None;
        unsafe {
            d3d11_cx
                .device
                .CreatePixelShader(
                    std::slice::from_raw_parts(
                        ps_blob.GetBufferPointer() as *const u8,
                        ps_blob.GetBufferSize() as usize,
                    ),
                    None,
                    Some(&mut ps),
                )
                .unwrap()
        };

        let mut layout_desc = Vec::new();
        let mut layout_debug = Vec::new();
        let mut strings: Vec<String> = Vec::new();
        let geom_desc_count: usize = mapping
            .geometries
            .inputs
            .iter()
            .map(|geom| slot_chunks(geom.slots).len())
            .sum();
        let inst_desc_count: usize = mapping
            .instances
            .inputs
            .iter()
            .map(|inst| slot_chunks(inst.slots).len())
            .sum();
        let total_desc_count = geom_desc_count + inst_desc_count;
        layout_desc.reserve(total_desc_count);
        strings.reserve(mapping.geometries.inputs.len() + mapping.instances.inputs.len());

        let mut geom_sem_index = 0usize;
        for geom in &mapping.geometries.inputs {
            strings.push(format!("GEOM{}\0", index_to_char(geom_sem_index)));
            let semantic_name = PCSTR(strings.last().unwrap().as_ptr());
            let mut slot_offset = 0usize;
            for (semantic_chunk_index, chunk_slots) in slot_chunks(geom.slots).into_iter().enumerate()
            {
                layout_desc.push(D3D11_INPUT_ELEMENT_DESC {
                    SemanticName: semantic_name,
                    SemanticIndex: semantic_chunk_index as u32,
                    Format: slots_to_dxgi_format(chunk_slots, geom.attr_format),
                    InputSlot: 0,
                    AlignedByteOffset: ((geom.offset + slot_offset) * 4) as u32,
                    InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                    InstanceDataStepRate: 0,
                });
                layout_debug.push(format!(
                    "{}{} slot={} slots={} byte_off={}",
                    strings.last().unwrap().trim_end_matches('\0'),
                    semantic_chunk_index,
                    0,
                    chunk_slots,
                    (geom.offset + slot_offset) * 4
                ));
                slot_offset += chunk_slots;
            }
            geom_sem_index += 1;
        }

        let mut inst_sem_index = 0usize;
        for inst in &mapping.instances.inputs {
            strings.push(format!("INST{}\0", index_to_char(inst_sem_index)));
            let semantic_name = PCSTR(strings.last().unwrap().as_ptr());
            let mut slot_offset = 0usize;
            for (semantic_chunk_index, chunk_slots) in slot_chunks(inst.slots).into_iter().enumerate()
            {
                layout_desc.push(D3D11_INPUT_ELEMENT_DESC {
                    SemanticName: semantic_name,
                    SemanticIndex: semantic_chunk_index as u32,
                    Format: slots_to_dxgi_format(chunk_slots, inst.attr_format),
                    InputSlot: 1,
                    AlignedByteOffset: ((inst.offset + slot_offset) * 4) as u32,
                    InputSlotClass: D3D11_INPUT_PER_INSTANCE_DATA,
                    InstanceDataStepRate: 1,
                });
                layout_debug.push(format!(
                    "{}{} slot={} slots={} byte_off={}",
                    strings.last().unwrap().trim_end_matches('\0'),
                    semantic_chunk_index,
                    1,
                    chunk_slots,
                    (inst.offset + slot_offset) * 4
                ));
                slot_offset += chunk_slots;
            }
            inst_sem_index += 1;
        }

        let mut input_layout = None;
        let input_layout_res = unsafe {
            d3d11_cx.device.CreateInputLayout(
                &layout_desc,
                std::slice::from_raw_parts(
                    vs_blob.GetBufferPointer() as *const u8,
                    vs_blob.GetBufferSize() as usize,
                ),
                Some(&mut input_layout),
            )
        };
        if let Err(err) = input_layout_res {
            println!("Cannot create input layout: {:?}", err);
            println!("Input layout descriptors:");
            for item in &layout_debug {
                println!("  {}", item);
            }
            if std::env::var("MAKEPAD_D3D11_DUMP_HLSL").is_ok() {
                println!("HLSL source\n{}", split_source(&hlsl));
            } else {
                println!("Set MAKEPAD_D3D11_DUMP_HLSL=1 to dump full HLSL source.");
            }
            std::process::exit(1);
        }

        let live_uniforms = D3d11Buffer::default();
        let const_table_uniforms = D3d11Buffer::default();
        let mut scope_uniforms = D3d11Buffer::default();
        if !mapping.scope_uniforms_buf.is_empty() {
            scope_uniforms.update_with_f32_constant_data(d3d11_cx, &mapping.scope_uniforms_buf);
        }

        // Look up buffer IDs from shader output bindings by Pod type name
        let draw_call_uniform_buffer_id = bindings
            .get_by_type_name(id!(DrawCallUniforms))
            .map(|i| i as u32);
        let pass_uniform_buffer_id = bindings
            .get_by_type_name(id!(DrawPassUniforms))
            .map(|i| i as u32);
        let draw_list_uniform_buffer_id = bindings
            .get_by_type_name(id!(DrawListUniforms))
            .map(|i| i as u32);
        // dyn_uniform_buffer_id uses the IoUniform cbuffer at register b2
        let dyn_uniform_buffer_id = Some(2);
        let scope_uniform_buffer_id = bindings.scope_uniform_buffer_index.map(|i| i as u32);

        Some(Self {
            hlsl,
            const_table_uniforms,
            live_uniforms,
            scope_uniforms,
            pixel_shader: ps.unwrap(),
            vertex_shader: vs.unwrap(),
            pixel_shader_blob: ps_blob,
            vertex_shader_blob: vs_blob,
            input_layout: input_layout.unwrap(),
            draw_call_uniform_buffer_id,
            pass_uniform_buffer_id,
            draw_list_uniform_buffer_id,
            dyn_uniform_buffer_id,
            scope_uniform_buffer_id,
        })
    }
}
