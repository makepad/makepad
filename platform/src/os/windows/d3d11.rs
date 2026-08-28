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
        windows::win32_app::{
            try_with_win32_app, with_win32_app, FALSE, TRUE,
        },
        windows::win32_window::Win32Window,
    },
    script::vm::*,
    texture::Texture,
    texture::{CxTexture, TextureFormat, TextureId, TexturePixel, TextureUpdated},
    window::WindowId,
    windows::{
        core::{
            //ComInterface,
            Interface,
            PCSTR,
            PCWSTR,
        },
        Win32::{
            Foundation::{CloseHandle, HANDLE, HMODULE, S_FALSE, WAIT_TIMEOUT},
            Graphics::{
                Direct3D::{
                    Fxc::D3DCompile, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
                    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0,
                    D3D_SRV_DIMENSION_TEXTURECUBE,
                },
                Direct3D11::{
                    D3D11CreateDevice, ID3D11BlendState, ID3D11Buffer, ID3D11DepthStencilState,
                    ID3D11DepthStencilView, ID3D11Device, ID3D11Device1, ID3D11DeviceContext, ID3D11InputLayout,
                    ID3D11PixelShader, ID3D11Query, ID3D11RasterizerState, ID3D11RenderTargetView,
                    ID3D11Resource, ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11VertexShader,
                    D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_DEPTH_STENCIL, D3D11_BIND_FLAG,
                    D3D11_BIND_INDEX_BUFFER, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
                    D3D11_BIND_VERTEX_BUFFER, D3D11_BLEND_DESC, D3D11_BLEND_INV_SRC_ALPHA,
                    D3D11_BLEND_ONE, D3D11_BLEND_OP_ADD, D3D11_BOX, D3D11_BUFFER_DESC, D3D11_CLEAR_DEPTH,
                    D3D11_CLEAR_STENCIL, D3D11_COLOR_WRITE_ENABLE_ALL, D3D11_COMPARISON_ALWAYS,
                    D3D11_COMPARISON_LESS_EQUAL, D3D11_CPU_ACCESS_WRITE, D3D11_CREATE_DEVICE_FLAG,
                    D3D11_CULL_BACK, D3D11_CULL_NONE, D3D11_DEPTH_STENCILOP_DESC,
                    D3D11_DEPTH_STENCIL_DESC, D3D11_DEPTH_STENCIL_VIEW_DESC,
                    D3D11_DEPTH_WRITE_MASK_ALL, D3D11_DEPTH_WRITE_MASK_ZERO,
                    D3D11_DSV_DIMENSION_TEXTURE2D, D3D11_FILL_SOLID, D3D11_INPUT_ELEMENT_DESC,
                    D3D11_INPUT_PER_INSTANCE_DATA, D3D11_INPUT_PER_VERTEX_DATA,
                    D3D11_MAP, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_WRITE_DISCARD, D3D11_QUERY_DESC,
                    D3D11_QUERY_EVENT, D3D11_RASTERIZER_DESC, D3D11_RENDER_TARGET_BLEND_DESC,
                    D3D11_RENDER_TARGET_VIEW_DESC, D3D11_RENDER_TARGET_VIEW_DESC_0,
                    D3D11_RESOURCE_MISC_FLAG, D3D11_RESOURCE_MISC_TEXTURECUBE,
                    D3D11_RTV_DIMENSION_TEXTURE2DARRAY, D3D11_SDK_VERSION,
                    D3D11_SHADER_RESOURCE_VIEW_DESC, D3D11_SHADER_RESOURCE_VIEW_DESC_0,
                    D3D11_STENCIL_OP_REPLACE, D3D11_SUBRESOURCE_DATA, D3D11_TEX2D_ARRAY_RTV,
                    D3D11_TEXCUBE_SRV, D3D11_TEXTURE2D_DESC, D3D11_USAGE, D3D11_USAGE_DEFAULT,
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
                        DXGI_FORMAT_R8G8B8A8_UNORM,
                        DXGI_FORMAT_R8G8_UNORM,
                        DXGI_FORMAT_R8_UNORM,
                        DXGI_SAMPLE_DESC,
                    },
                    CreateDXGIFactory2, IDXGIFactory2, IDXGIKeyedMutex, IDXGIResource,
                    IDXGIResource1, IDXGISwapChain, IDXGISwapChain1, IDXGISwapChain2,
                    DXGI_CREATE_FACTORY_FLAGS,
                    DXGI_ERROR_WAS_STILL_DRAWING, DXGI_FRAME_STATISTICS, DXGI_PRESENT,
                    DXGI_PRESENT_DO_NOT_WAIT, DXGI_RGBA,
                    DXGI_SCALING_NONE, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
                    DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT,
                    DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT,
                },
            },
            System::{
                Com::CoTaskMemFree,
                Threading::WaitForSingleObject,
            },
            UI::Shell::{FOLDERID_LocalAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath},
        },
    },
};
use std::cell::Cell;

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
        let draw_order_len = self.draw_lists[draw_list_id].draw_item_order_len();

        {
            let draw_list = &mut self.draw_lists[draw_list_id];
            draw_list
                .os
                .draw_list_uniforms
                .update_with_f32_constant_data(d3d11_cx, draw_list.draw_list_uniforms.as_slice());
        }

        for order_index in 0..draw_order_len {
            let Some(draw_item_id) =
                self.draw_lists[draw_list_id].draw_item_id_at_order_index(order_index)
            else {
                continue;
            };
            if let Some(sub_list_id) = self.draw_lists[draw_list_id].draw_items[draw_item_id]
                .kind
                .sub_list()
            {
                let child_resets_zbias = self.draw_lists[sub_list_id].reset_zbias;
                let mut child_zbias = 0.0f32;
                self.render_view(
                    pass_id,
                    sub_list_id,
                    if child_resets_zbias {
                        &mut child_zbias
                    } else {
                        zbias
                    },
                    zbias_step,
                    d3d11_cx,
                );
            } else {
                let draw_list = &mut self.draw_lists[draw_list_id];
                let draw_item = &mut draw_list.draw_items[draw_item_id];
                let draw_call = if let Some(draw_call) = draw_item.kind.draw_call_mut() {
                    draw_call
                } else {
                    continue;
                };
                // The zbias is the fragment depth, and it is handed out from the global draw
                // order. It must advance for every draw call in the tree, ahead of the early-outs
                // below, or the sequence would depend on which draw calls happen to be dirty this
                // frame rather than on the draw tree alone.
                let zbias_changed = draw_call.draw_call_uniforms.set_zbias(*zbias);
                *zbias += zbias_step;

                // A cached draw call (one whose draw list was not redrawn this frame) has
                // `uniforms_dirty` unset, but its zbias still shifts whenever any draw list drawn
                // before it grows or shrinks. Uploading only on `uniforms_dirty` would leave the
                // GPU with a stale depth, and the LESS_EQUAL depth test then rejects the draw
                // call. `buffer.is_none()` covers a buffer that was never uploaded at all.
                if draw_call.uniforms_dirty
                    || zbias_changed
                    || draw_item.os.draw_call_uniforms.buffer.is_none()
                {
                    draw_call.uniforms_dirty = false;
                    draw_item
                        .os
                        .draw_call_uniforms
                        .update_with_f32_constant_data(
                            d3d11_cx,
                            draw_call.draw_call_uniforms.as_slice(),
                        );
                }

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

                if sh.mapping.flags.debug_draw {
                    CxDrawShaderMapping::debug_dump_shader_draw_call(
                        "d3d11",
                        draw_item_id,
                        sh,
                        draw_call,
                        draw_item.instances.as_ref().unwrap(),
                        instances as usize,
                    );
                }

                let geometry_id = if let Some(geometry_id) = draw_call.geometry_id {
                    geometry_id
                } else {
                    continue;
                };

                let geometry = &mut self.geometries[geometry_id];

                if geometry.dirty_indices {
                    geometry
                        .os
                        .geom_ibuf
                        .update_with_u32_index_data(d3d11_cx, &geometry.indices);
                    geometry.dirty_indices = false;
                }
                if geometry.dirty_vertices {
                    geometry
                        .os
                        .geom_vbuf
                        .update_with_f32_vertex_data(d3d11_cx, &geometry.vertices);
                    geometry.dirty_vertices = false;
                }
                geometry.dirty = geometry.dirty_vertices || geometry.dirty_indices;

                unsafe {
                    d3d11_cx.context.VSSetShader(&shp.vertex_shader, None);
                    d3d11_cx.context.PSSetShader(&shp.pixel_shader, None);
                    d3d11_cx
                        .context
                        .IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
                    d3d11_cx.context.IASetInputLayout(&shp.input_layout);

                    let depth_stencil_state = if draw_call.options.depth_write {
                        self.passes[pass_id].os.depth_stencil_state_write.as_ref()
                    } else {
                        self.passes[pass_id]
                            .os
                            .depth_stencil_state_no_write
                            .as_ref()
                    };
                    if let Some(depth_stencil_state) = depth_stencil_state {
                        d3d11_cx
                            .context
                            .OMSetDepthStencilState(depth_stencil_state, 0);
                    }
                    let raster_state = if draw_call.options.backface_culling {
                        self.passes[pass_id].os.raster_state_backface_cull.as_ref()
                    } else {
                        self.passes[pass_id].os.raster_state_no_cull.as_ref()
                    };
                    if let Some(raster_state) = raster_state {
                        d3d11_cx.context.RSSetState(raster_state);
                    }

                    // A geometry whose buffers could not be built — the device died between
                    // frames, so `create_buffer_or_update` bailed — skips its draw call
                    // instead of panicking a few lines before `DrawIndexedInstanced`. Binding
                    // a null vertex buffer would be worse than either: it draws nothing and
                    // reports nothing. The recovery sweep is what puts these back.
                    let (Some(geom_ibuf), Some(geom_vbuf)) = (
                        geometry.os.geom_ibuf.buffer.clone(),
                        geometry.os.geom_vbuf.buffer.clone(),
                    ) else {
                        debug_assert!(
                            d3d11_cx.device_lost.get(),
                            "geometry buffers missing while the device is healthy"
                        );
                        continue;
                    };
                    d3d11_cx
                        .context
                        .IASetIndexBuffer(&geom_ibuf, DXGI_FORMAT_R32_UINT, 0);

                    let geom_slots = sh.mapping.geometries.total_slots;
                    let inst_slots = sh.mapping.instances.total_slots;
                    let strides = [(geom_slots * 4) as u32, (inst_slots * 4) as u32];
                    let offsets = [0u32, 0u32];
                    let buffers = [Some(geom_vbuf), draw_item.os.inst_vbuf.buffer.clone()];
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
                                let clear_buffers = [None];
                                d3d11_cx
                                    .context
                                    .VSSetConstantBuffers(index, Some(&clear_buffers));
                                d3d11_cx
                                    .context
                                    .PSSetConstantBuffers(index, Some(&clear_buffers));
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
                    for (slot, idx) in shp.custom_uniform_buffer_ids.iter().enumerate() {
                        if let Some(uniform_buffer) = draw_call.uniform_buffer_slots[slot].as_ref()
                        {
                            let cx_uniform_buffer =
                                &mut self.uniform_buffers[uniform_buffer.uniform_buffer_id()];
                            cx_uniform_buffer
                                .os
                                .buffer
                                .update_with_constant_bytes(d3d11_cx, &cx_uniform_buffer.data);
                            buffer_slot(d3d11_cx, *idx, &cx_uniform_buffer.os.buffer.buffer);
                        } else {
                            buffer_slot(d3d11_cx, *idx, &None);
                        }
                    }
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

                // Cross-process shared textures (studio RunView) are backed by a keyed
                // mutex. This consumer device must HOLD it while sampling so it waits on
                // the producer's writes (GPU-timeline) and sees coherent pixels; released
                // after the draw call below.
                let mut acquired_mutexes: Vec<IDXGIKeyedMutex> = Vec::new();
                for i in 0..sh.mapping.textures.len() {
                    let texture_id = if let Some(texture) = &draw_call.texture_slots[i] {
                        texture.texture_id()
                    } else {
                        let clear_srvs = [None];
                        unsafe {
                            d3d11_cx
                                .context
                                .PSSetShaderResources(i as u32, Some(&clear_srvs));
                            d3d11_cx
                                .context
                                .VSSetShaderResources(i as u32, Some(&clear_srvs));
                        }
                        continue;
                    };

                    let cxtexture = &mut self.textures[texture_id];

                    if cxtexture.format.is_shared() {
                        cxtexture.update_shared_texture(&d3d11_cx.device);
                        if let Some(km) = &cxtexture.os.keyed_mutex {
                            let km = km.clone();
                            unsafe {
                                let _ = (Interface::vtable(&km).AcquireSync)(
                                    Interface::as_raw(&km),
                                    0,
                                    2000,
                                );
                            }
                            acquired_mutexes.push(km);
                        }
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
                            let clear_srvs = [None];
                            d3d11_cx
                                .context
                                .PSSetShaderResources(i as u32, Some(&clear_srvs));
                            d3d11_cx
                                .context
                                .VSSetShaderResources(i as u32, Some(&clear_srvs));
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
                // Release keyed mutexes acquired for shared textures in this draw call.
                for km in &acquired_mutexes {
                    unsafe {
                        let _ = (Interface::vtable(km).ReleaseSync)(Interface::as_raw(km), 0);
                    }
                }
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
        if !self.passes[pass_id].keep_camera_matrix {
            self.passes[pass_id].set_ortho_matrix(pass_rect.pos, pass_rect.size);
        }
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
                let render_target = if let Some(cube_face) = color_texture.cube_face {
                    cxtexture.os.render_target_face_views[cube_face as usize].clone()
                } else {
                    cxtexture.os.render_target_view.clone()
                };
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
            let depth_stencil_view = cxtexture.os.depth_stencil_view.clone().unwrap();
            let is_initial = cxtexture.take_initial();

            match self.passes[pass_id].clear_depth {
                DrawPassClearDepth::InitWith(depth_clear) => {
                    if is_initial {
                        unsafe {
                            d3d11_cx.context.ClearDepthStencilView(
                                &depth_stencil_view,
                                D3D11_CLEAR_DEPTH.0 as u32 | D3D11_CLEAR_STENCIL.0 as u32,
                                depth_clear,
                                0,
                            )
                        }
                    }
                }
                DrawPassClearDepth::ClearWith(depth_clear) => unsafe {
                    d3d11_cx.context.ClearDepthStencilView(
                        &depth_stencil_view,
                        D3D11_CLEAR_DEPTH.0 as u32 | D3D11_CLEAR_STENCIL.0 as u32,
                        depth_clear,
                        0,
                    )
                },
            }
            unsafe {
                d3d11_cx
                    .context
                    .OMSetRenderTargets(Some(&color_textures), Some(&depth_stencil_view))
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

    /// Renders the pass and presents it to the window. Returns whether a frame was
    /// actually presented; `false` means it was dropped and the caller must re-present.
    pub fn draw_pass_to_window(
        &mut self,
        pass_id: DrawPassId,
        vsync: bool,
        d3d11_window: &mut D3d11Window,
        d3d11_cx: &D3d11Cx,
    ) -> bool {
        // let time1 = Cx::profile_time_ns();
        let draw_list_id = self.passes[pass_id].main_draw_list_id.unwrap();

        // Pace the CPU to the display refresh by waiting on this swap chain's
        // frame-latency waitable before building the frame. The waitable is a
        // counted semaphore replenished once per retired Present, so each wait
        // must be paired with the vsync Present below.
        //
        // The beat in the event loop normally does that wait for us — one wait
        // covering every window plus input — and dispatches a scoped tick for the
        // window it woke on, holding the credit it took. Waiting AGAIN here would
        // take a second credit and stall a whole refresh, so a window that already
        // holds one goes straight to the present that spends it. Only the paths
        // the beat does not cover actually wait here: popups (no waitable), a live
        // resize (unpaced), and the unscoped heartbeat tick.
        let window_id = d3d11_window.window_id;
        // A `--remote` `/g` grab (or a studio screenshot) is only ever answered by
        // a pass that actually renders, so a pending one has to survive every
        // pacing skip below — otherwise an occluded or stalled window turns the
        // request into a ten-second timeout instead of a PNG.
        let capture_pending = self.has_pending_window_screenshot(window_id);
        let holds_credit =
            try_with_win32_app(|app| app.has_beat_credit(window_id)).unwrap_or(false);
        let mut latency_wait_timed_out = false;
        if vsync
            && !holds_credit
            && !d3d11_window.is_in_resize
            && !d3d11_window.frame_latency_waitable.is_invalid()
        {
            unsafe {
                latency_wait_timed_out =
                    WaitForSingleObject(d3d11_window.frame_latency_waitable, 33) == WAIT_TIMEOUT;
            }
            if !latency_wait_timed_out {
                try_with_win32_app(|app| app.take_beat_credit(window_id));
            }
        }
        if latency_wait_timed_out {
            // The compositor is not retiring this window's presents (occluded,
            // minimized, or DWM stalled). The old code pushed a DO_NOT_WAIT frame
            // anyway, so a hidden window rebuilt and re-presented its whole scene
            // every 33 ms forever. Skip the frame instead — the caller keeps the
            // pass dirty — and only spend a frame probing now and then, since the
            // stall can end without anything telling us.
            let now = std::time::Instant::now();
            let since = *d3d11_window.latency_timeout_since.get_or_insert(now);
            if now.duration_since(since) < LATENCY_TIMEOUT_PROBE_INTERVAL {
                // Unless a capture is waiting on this window: render it anyway and
                // leave the probe clock alone — this frame belongs to the grab, not
                // to the periodic probe, so it must not postpone the next one.
                if !capture_pending {
                    return false;
                }
            } else {
                d3d11_window.latency_timeout_since = Some(now);
            }
        } else {
            d3d11_window.latency_timeout_since = None;
        }

        // Serialize with FFmpeg D3D11VA when sharing Makepad's device (ZC video).
        let mut presented = false;
        crate::gpu_texture::with_media_d3d11_lock(|| {
            self.setup_pass_render_targets(pass_id, &d3d11_window.render_target_view, d3d11_cx);

            let mut zbias = 0.0;
            let zbias_step = self.passes[pass_id].zbias_step;

            self.render_view(pass_id, draw_list_id, &mut zbias, zbias_step, d3d11_cx);
            // Read the frame back BEFORE it flips: the chain is FLIP_DISCARD, so
            // the back buffer's contents are undefined the moment `Present` takes
            // it. Cheap when nothing asked for a capture (one Vec check).
            if capture_pending {
                self.capture_window_screenshot(d3d11_window, d3d11_cx);
            }
            presented = d3d11_window.present(vsync, latency_wait_timed_out);
        });
        // A frame went to `Present`, so the credit is spent — whether or not the
        // compositor kept it. Assuming it spent when it was not only costs one
        // paced wait; assuming it held when it was spent would remove the pacing
        // for this window entirely, so err on the side of waiting again.
        try_with_win32_app(|app| app.spend_beat_credit(window_id));
        // Reveal the window only once a frame reached the compositor; showing it
        // earlier would flash an uncomposited black window.
        if presented && d3d11_window.first_draw {
            d3d11_window.win32_window.show();
            d3d11_window.first_draw = false;
        }
        //println!("{}", (Cx::profile_time_ns() - time1)as f64 / 1000.0);
        presented
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
        let cache_dir = shader_cache_dir();

        // Step 1: adopt any background compiles that finished since the last
        // call. The worker thread writes the DXBC into the on-disk cache
        // before sending the completion, so CxOsDrawShader::new takes the
        // cache-hit path (disk read + D3D11 object creation — a few ms).
        // The scoped block below keeps the immutable borrow of
        // draw_shaders.shaders short so we can mutate it afterwards to set
        // os_shader_id; that avoids the explicit mapping/bindings clones
        // an earlier revision used.
        // Spread D3D11 shader-object creation across frames: creating many shader objects in one
        // frame (e.g. when scrolling brings in lots of new content types at once) caused a visible
        // hitch (tens of ms). Cap how many we create per call; the rest are deferred to following
        // frames, and widgets whose shader isn't ready yet skip their draw (the existing
        // `os_shader_id.is_none()` guard) and materialize a frame or two later.
        const SHADER_CREATE_BUDGET: usize = 4;
        let (ready_async, has_more_async) =
            self.os.async_hlsl_compile.drain_ready(SHADER_CREATE_BUDGET);
        let mut created = ready_async.len();
        let mut any_async_ready = false;
        for result in ready_async {
            any_async_ready = true;
            if let Err(msg) = &result.vs_status {
                crate::error!(
                    "Background vertex-shader compile failed for shader id {}: {}",
                    result.shader_id,
                    msg
                );
                continue;
            }
            if let Err(msg) = &result.ps_status {
                crate::error!(
                    "Background pixel-shader compile failed for shader id {}: {}",
                    result.shader_id,
                    msg
                );
                continue;
            }
            let shader_id = result.shader_id;
            let shp = {
                let cx_shader = &self.draw_shaders.shaders[shader_id];
                let CxDrawShaderCode::Combined { code } = &cx_shader.mapping.code else {
                    continue;
                };
                CxOsDrawShader::new(
                    d3d11_cx,
                    code,
                    cache_dir,
                    &cx_shader.mapping,
                    &cx_shader.mapping.uniform_buffer_bindings,
                )
            };
            if let Some(shp) = shp {
                let cx_shader = &mut self.draw_shaders.shaders[shader_id];
                cx_shader.os_shader_id = Some(self.draw_shaders.os_shaders.len());
                self.draw_shaders.os_shaders.push(shp);
            }
        }
        if any_async_ready || has_more_async {
            // Widgets that skipped their draw call because the shader wasn't ready need one more
            // redraw to materialize now that it is (or once the deferred backlog is created).
            self.redraw_all();
        }

        if self.draw_shaders.compile_set.is_empty() {
            return;
        }
        let compile_set = std::mem::take(&mut self.draw_shaders.compile_set);

        // Step 2: partition by cache state, computing the cache key once.
        //
        // Cache hit  → sync path: disk read + D3D11 object creation, a few
        //              ms total. Faster than thread/channel overhead.
        // Cache miss → async path: D3DCompile can burn 100ms-multiple
        //              seconds per shader, so it must not block the frame.
        // `async_compile: true` (the SLUG helper) still forces async even
        // on a cache hit, matching the Linux flag semantics — the one-frame
        // latency on warm cache is acceptable, and this keeps behavior
        // consistent across platforms.
        let mut async_items: Vec<(usize, u64)> = Vec::new();
        let mut sync_ids: Vec<usize> = Vec::new();
        for id in compile_set {
            let sh = &self.draw_shaders.shaders[id];
            let code = match &sh.mapping.code {
                CxDrawShaderCode::Combined { code } => code,
                CxDrawShaderCode::Separate { .. } => {
                    crate::error!("D3D11 does not support separate vertex/fragment sources");
                    continue;
                }
            };
            let cache_key = hlsl_cache_key(code);
            let cached = shader_bytes_cached(cache_dir, cache_key);
            let force_async = sh.mapping.flags.async_compile;
            if force_async || !cached {
                async_items.push((id, cache_key));
            } else {
                sync_ids.push(id);
            }
        }

        // Step 3: dispatch background compiles. The window presents this
        // frame without waiting; widgets whose shader isn't ready skip
        // their draw call via the `sh.os_shader_id.is_none()` guard in
        // render_view. When workers finish, the next hlsl_compile_shaders
        // call drains them and triggers a redraw.
        for (id, cache_key) in async_items {
            let hlsl = {
                let sh = &self.draw_shaders.shaders[id];
                let CxDrawShaderCode::Combined { code } = &sh.mapping.code else {
                    continue;
                };
                code.clone()
            };
            self.os
                .async_hlsl_compile
                .spawn(id, hlsl, cache_key, cache_dir);
        }

        // Step 4: serial D3D11 object creation for the cache-hit shaders, up to the per-frame
        // budget. Any beyond the budget are put back into compile_set for a following frame.
        for draw_shader_id in sync_ids {
            if created >= SHADER_CREATE_BUDGET {
                self.draw_shaders.compile_set.insert(draw_shader_id);
                continue;
            }
            created += 1;
            let shp = {
                let cx_shader = &self.draw_shaders.shaders[draw_shader_id];
                if cx_shader.mapping.flags.debug_code {
                    if let CxDrawShaderCode::Combined { code } = &cx_shader.mapping.code {
                        crate::log!("{}", code);
                    }
                }
                let CxDrawShaderCode::Combined { code } = &cx_shader.mapping.code else {
                    continue;
                };
                CxOsDrawShader::new(
                    d3d11_cx,
                    code,
                    cache_dir,
                    &cx_shader.mapping,
                    &cx_shader.mapping.uniform_buffer_bindings,
                )
            };
            if let Some(shp) = shp {
                let cx_shader = &mut self.draw_shaders.shaders[draw_shader_id];
                cx_shader.os_shader_id = Some(self.draw_shaders.os_shaders.len());
                self.draw_shaders.os_shaders.push(shp);
            } else {
                // `compile_set` was drained into `sync_ids`, so a shader dropped here is never
                // asked for again and everything drawn with it silently stops rendering for
                // the life of the process. Creation fails for a whole frame's worth of shaders
                // when the device dies mid-compile, so put it back and let the next frame,
                // against a rebuilt device, create it.
                self.draw_shaders.compile_set.insert(draw_shader_id);
            }
        }

        // If work was deferred (either async backlog or budgeted-out sync shaders), request a
        // redraw so the next frame creates the rest and the skipped widgets re-materialize.
        if has_more_async || !self.draw_shaders.compile_set.is_empty() {
            self.redraw_all();
        }
    }

    pub fn share_texture_for_presentable_image(&mut self, texture: &Texture) -> u64 {
        let cxtexture = &mut self.textures[texture.texture_id()];
        cxtexture.update_shared_texture(self.os.d3d11_device.as_ref().unwrap());
        cxtexture.os.shared_handle.0 as u64
    }

    /// Acquire the cross-process keyed mutex of a shared texture (key 0). A no-op for
    /// textures without a keyed mutex (non-shared). The 2s timeout guards against a peer
    /// that died holding the mutex; on timeout AcquireSync still returns a success HRESULT
    /// (WAIT_TIMEOUT), so callers always pair this with a `release` and never deadlock.
    pub fn shared_texture_keyed_acquire(&self, texture: &Texture) {
        if let Some(km) = &self.textures[texture.texture_id()].os.keyed_mutex {
            // The stripped windows bindings only expose the `_Impl` (server) side of
            // IDXGIKeyedMutex, so call AcquireSync through the vtable like `is_gpu_done`
            // does for ID3D11DeviceContext::GetData.
            unsafe {
                let _ = (Interface::vtable(km).AcquireSync)(Interface::as_raw(km), 0, 2000);
            }
        }
    }

    /// Release the cross-process keyed mutex of a shared texture (key 0).
    pub fn shared_texture_keyed_release(&self, texture: &Texture) {
        if let Some(km) = &self.textures[texture.texture_id()].os.keyed_mutex {
            unsafe {
                let _ = (Interface::vtable(km).ReleaseSync)(Interface::as_raw(km), 0);
            }
        }
    }

    /// True when a pending screenshot request can be answered by the pass that
    /// belongs to `window_id` — a `--remote` `/g` grab targeted at it, or an
    /// untargeted studio / `capture_next_frame_to_file` request.
    ///
    /// Deliberately non-destructive, unlike
    /// `take_studio_screenshot_request_ids_for_window`: the pacing skips decide
    /// whether to render a frame at all, and they have to see the request while
    /// it is still pending.
    pub(crate) fn has_pending_window_screenshot(&self, window_id: WindowId) -> bool {
        if self.screenshot_requests.is_empty() {
            return false;
        }
        let window_id = Some(window_id.id());
        self.screenshot_requests.iter().any(|request| {
            request.kind_id == 0
                && crate::remote::grab_targets_window(request.request_id, window_id)
        })
    }

    /// Answer the screenshot requests this window's pass can serve: staging-copy
    /// the swap-chain back buffer, map it, and hand the rows to the shared PNG
    /// path that `/g` grabs, studio screenshots and `capture_next_frame_to_file`
    /// all consume (`send_studio_screenshot_response`).
    ///
    /// Called from `draw_pass_to_window` after `render_view` and before
    /// `Present`. Ordering needs no fence of ours: `CopySubresourceRegion` is
    /// queued on the immediate context behind this pass's draw calls, and
    /// `Map(D3D11_MAP_READ)` blocks until that copy has retired — the same
    /// argument `debug_read_render_texture` already relies on.
    fn capture_window_screenshot(&mut self, d3d11_window: &D3d11Window, d3d11_cx: &D3d11Cx) {
        let request_ids = self
            .take_studio_screenshot_request_ids_for_window(0, Some(d3d11_window.window_id.id()));
        if request_ids.is_empty() {
            return;
        }
        // Every failure path below still answers the request with an empty PNG:
        // a grab that is never answered blocks its HTTP thread for the full
        // timeout, which reads as a hung app rather than a failed capture.
        let Some(swap_texture) = d3d11_window.swap_texture.clone() else {
            crate::error!("window capture: swap chain has no back buffer");
            Self::send_studio_screenshot_response(request_ids, 0, 0, Vec::new());
            return;
        };
        let Some((width, height, mut rgba)) = read_texture_rgba(&swap_texture, d3d11_cx) else {
            Self::send_studio_screenshot_response(request_ids, 0, 0, Vec::new());
            return;
        };
        // The chain is DXGI_ALPHA_MODE_IGNORE, so its alpha channel is not part
        // of the presented image. Ship it opaque rather than let a premultiplied
        // stray alpha make the PNG disagree with what is on the glass.
        for px in rgba.chunks_exact_mut(4) {
            px[3] = 255;
        }
        match Self::encode_rgba_as_png(width, height, &rgba) {
            Ok(png) => Self::send_studio_screenshot_response(request_ids, width, height, png),
            Err(err) => {
                crate::error!("window capture: {}", err);
                Self::send_studio_screenshot_response(request_ids, width, height, Vec::new());
            }
        }
    }

    /// Renderer-owned texture capture (see the metal backend): not
    /// implemented here — callers fall back to `debug_read_render_texture`
    /// (D3D11 immediate-context commands are ordered against the staging
    /// copy, so the sync path is safe).
    pub fn request_render_texture_capture(
        &mut self,
        _texture: &crate::texture::Texture,
    ) -> bool {
        false
    }

    #[allow(clippy::type_complexity)]
    pub fn take_render_texture_captures(
        &mut self,
    ) -> Vec<(crate::texture::TextureId, usize, usize, Vec<u8>)> {
        Vec::new()
    }

    /// CPU grab of a render target (thumbnail icons). Staging copy + Map.
    /// Returns packed BGRA8, same layout as the Metal readback.
    pub fn debug_read_render_texture(
        &mut self,
        texture: &Texture,
    ) -> Option<(usize, usize, Vec<u8>)> {
        let cxtexture = &self.textures[texture.texture_id()];
        let alloc = cxtexture.alloc.as_ref()?;
        let (width, height) = (alloc.width, alloc.height);
        if width == 0 || height == 0 {
            return None;
        }
        let src_tex = cxtexture.os.texture.clone()?;
        let device = self.os.d3d11_device.clone()?;
        unsafe {
            let context = device.GetImmediateContext().ok()?;
            let desc = D3D11_TEXTURE2D_DESC {
                Width: width as u32,
                Height: height as u32,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE(3),
                BindFlags: 0,
                CPUAccessFlags: 0x20000,
                MiscFlags: 0,
            };
            let mut staging: Option<ID3D11Texture2D> = None;
            device
                .CreateTexture2D(&desc, None, Some(&mut staging))
                .ok()?;
            let staging_res: ID3D11Resource = staging?.cast().ok()?;
            let src_res: ID3D11Resource = src_tex.cast().ok()?;
            context.CopySubresourceRegion(&staging_res, 0, 0, 0, 0, &src_res, 0, None);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context
                .Map(&staging_res, 0, D3D11_MAP(1), 0, Some(&mut mapped))
                .ok()?;
            let pitch = mapped.RowPitch as usize;
            let mut bytes = vec![0u8; width * height * 4];
            let src = mapped.pData as *const u8;
            for y in 0..height {
                let row = src.add(y * pitch);
                std::ptr::copy_nonoverlapping(
                    row,
                    bytes.as_mut_ptr().add(y * width * 4),
                    width * 4,
                );
            }
            context.Unmap(&staging_res, 0);
            Some((width, height, bytes))
        }
    }

    /// TEMP DIAGNOSTIC (strip before merge): CPU staging readback of a shared texture on
    /// THIS process's device — distinguishes "child renders black" / "coherence broken"
    /// from "studio display/layout bug". Copies the whole texture to a STAGING texture,
    /// maps it, and logs a 16x16 grid summary + key texels.
    pub fn debug_readback_shared_texture(&mut self, texture: &Texture, tag: &str) {
        let cxtexture = &self.textures[texture.texture_id()];
        let Some(src_tex) = cxtexture.os.texture.clone() else {
            crate::log!("WINDBG[{}]: readback: no os.texture", tag);
            return;
        };
        let keyed_mutex = cxtexture.os.keyed_mutex.clone();
        let (width, height) = if let TextureFormat::SharedBGRAu8 { width, height, .. } = &cxtexture.format {
            (*width as u32, *height as u32)
        } else {
            crate::log!("WINDBG[{}]: readback: not SharedBGRAu8", tag);
            return;
        };
        if width == 0 || height == 0 {
            return;
        }
        let Some(device) = self.os.d3d11_device.clone() else {
            crate::log!("WINDBG[{}]: readback: no device", tag);
            return;
        };
        unsafe {
            let context = match device.GetImmediateContext() {
                Ok(c) => c,
                Err(err) => {
                    crate::log!("WINDBG[{}]: GetImmediateContext failed {:?}", tag, err);
                    return;
                }
            };
            let desc = D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE(3), // D3D11_USAGE_STAGING (const stripped from bindings)
                BindFlags: 0,
                CPUAccessFlags: 0x20000, // D3D11_CPU_ACCESS_READ (const stripped from bindings)
                MiscFlags: 0,
            };
            let mut staging: Option<ID3D11Texture2D> = None;
            if let Err(err) = device.CreateTexture2D(&desc, None, Some(&mut staging)) {
                crate::log!("WINDBG[{}]: staging CreateTexture2D failed {:?}", tag, err);
                return;
            }
            let staging_res: ID3D11Resource = staging.unwrap().cast().unwrap();
            let src_res: ID3D11Resource = src_tex.cast().unwrap();
            let mut acq_hr = 0i32;
            if let Some(km) = &keyed_mutex {
                acq_hr = (Interface::vtable(km).AcquireSync)(Interface::as_raw(km), 0, 2000).0;
            }
            context.CopySubresourceRegion(&staging_res, 0, 0, 0, 0, &src_res, 0, None);
            if let Some(km) = &keyed_mutex {
                let _ = (Interface::vtable(km).ReleaseSync)(Interface::as_raw(km), 0);
            }
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            // D3D11_MAP_READ = 1 (const stripped from bindings)
            match context.Map(&staging_res, 0, D3D11_MAP(1), 0, Some(&mut mapped)) {
                Ok(()) => {
                    let px = |x: u32, y: u32| -> u32 {
                        let x = x.min(width - 1) as usize;
                        let y = y.min(height - 1) as usize;
                        let row = (mapped.pData as *const u8).add(y * mapped.RowPitch as usize)
                            as *const u32;
                        row.add(x).read_unaligned()
                    };
                    let mut nonzero = 0usize;
                    let mut distinct: Vec<u32> = Vec::new();
                    for gy in 0..16u32 {
                        for gx in 0..16u32 {
                            let v = px(gx * width / 16, gy * height / 16);
                            if v != 0 {
                                nonzero += 1;
                            }
                            if !distinct.contains(&v) && distinct.len() < 8 {
                                distinct.push(v);
                            }
                        }
                    }
                    crate::log!(
                        "WINDBG[{}]: readback {}x{} acq_hr={:#x} pitch={} nonzero={}/256 distinct={:08x?} tl={:08x} c={:08x} br={:08x}",
                        tag, width, height, acq_hr, mapped.RowPitch, nonzero, distinct,
                        px(2, 2), px(width / 2, height / 2), px(width - 3, height - 3)
                    );
                    context.Unmap(&staging_res, 0);
                }
                Err(err) => {
                    crate::log!("WINDBG[{}]: Map failed {:?}", tag, err);
                }
            }
        }
    }

    // HLSL shaders compile synchronously via `hlsl_compile_shaders`, so a shader
    // is "window-ready" iff its OS-level shader entry has been allocated.
    // Used by the shared SLUG helper path that also runs on Linux (where GL may
    // async-compile) to decide whether to draw or fall back to raster.
    pub fn is_draw_shader_window_ready(&self, shader_id: DrawShaderId) -> bool {
        self.draw_shaders.shaders[shader_id.index]
            .os_shader_id
            .is_some()
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
        TexturePixel::VideoYuvPlane => DXGI_FORMAT_R8_UNORM,
        TexturePixel::VideoExternal => DXGI_FORMAT_B8G8R8A8_UNORM,
        TexturePixel::VideoGlMemoryRgba => DXGI_FORMAT_R8G8B8A8_UNORM,
        TexturePixel::VideoRgbaHardwareBuffer => DXGI_FORMAT_R8G8B8A8_UNORM,
    }
}

/// Calls DwmFlush to synchronize with the Desktop Window Manager compositor.
/// This blocks until DWM has completed its current composition cycle, ensuring
/// that a just-presented swap chain frame is picked up before the next desktop
/// repaint. We ignore errors (e.g. DWM disabled on remote desktop sessions).
fn dwm_flush() {
    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmFlush() -> i32;
    }
    unsafe {
        let _ = DwmFlush();
    }
}

/// Staging-copy + `Map` readback of a render target or swap-chain back buffer,
/// returned as `(width, height, packed top-down RGBA8 rows)` — the layout
/// `Cx::encode_rgba_as_png` and every other backend's screenshot path expect.
/// Returns `None` (having logged) on any D3D failure.
///
/// The staging copy is what makes the read safe: a back buffer is
/// `D3D11_USAGE_DEFAULT` and cannot be mapped, so it is copied into a
/// `D3D11_USAGE_STAGING` twin with CPU read access, and the map blocks until
/// that copy retires. `RowPitch` is honoured — the driver pads rows to its own
/// alignment and is under no obligation to match `width * 4`.
fn read_texture_rgba(src: &ID3D11Texture2D, d3d11_cx: &D3d11Cx) -> Option<(u32, u32, Vec<u8>)> {
    unsafe {
        // Physical pixels straight from the texture, so a DPI-scaled window
        // reports the size it actually rendered rather than a logical one.
        let mut src_desc = D3D11_TEXTURE2D_DESC::default();
        src.GetDesc(&mut src_desc);
        let (width, height) = (src_desc.Width, src_desc.Height);
        if width == 0 || height == 0 {
            return None;
        }
        // Channel order of the copied bytes. Window chains are created BGRA8;
        // accept the RGBA8 variant too so an offscreen target reads back
        // correctly instead of with red and blue swapped.
        let swap_rb = if src_desc.Format == DXGI_FORMAT_B8G8R8A8_UNORM {
            true
        } else if src_desc.Format == DXGI_FORMAT_R8G8B8A8_UNORM {
            false
        } else {
            crate::error!(
                "window capture: unsupported back buffer format {}",
                src_desc.Format.0
            );
            return None;
        };
        // A staging destination must otherwise describe the same surface, or
        // CopySubresourceRegion silently does nothing.
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: src_desc.Format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE(3), // D3D11_USAGE_STAGING (const stripped from bindings)
            BindFlags: 0,
            CPUAccessFlags: 0x20000, // D3D11_CPU_ACCESS_READ (const stripped from bindings)
            MiscFlags: 0,
        };
        let mut staging: Option<ID3D11Texture2D> = None;
        if let Err(err) = d3d11_cx
            .device
            .CreateTexture2D(&desc, None, Some(&mut staging))
        {
            crate::error!("window capture: CreateTexture2D(staging) failed: {}", err);
            return None;
        }
        let staging_res: ID3D11Resource = staging?.cast().ok()?;
        let src_res: ID3D11Resource = src.cast().ok()?;
        d3d11_cx
            .context
            .CopySubresourceRegion(&staging_res, 0, 0, 0, 0, &src_res, 0, None);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        if let Err(err) = d3d11_cx.context.Map(
            &staging_res,
            0,
            D3D11_MAP(1), // D3D11_MAP_READ (const stripped from bindings)
            0,
            Some(&mut mapped),
        ) {
            crate::error!("window capture: Map(staging) failed: {}", err);
            return None;
        }
        let pitch = mapped.RowPitch as usize;
        let row_bytes = width as usize * 4;
        let mut out = vec![0u8; row_bytes * height as usize];
        let src_ptr = mapped.pData as *const u8;
        for y in 0..height as usize {
            std::ptr::copy_nonoverlapping(
                src_ptr.add(y * pitch),
                out.as_mut_ptr().add(y * row_bytes),
                row_bytes,
            );
        }
        d3d11_cx.context.Unmap(&staging_res, 0);
        if swap_rb {
            for px in out.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
        }
        Some((width, height, out))
    }
}

/// `Present` succeeded but the window is fully occluded, so nothing reached the
/// screen and DWM will stop retiring our presents. It is a SUCCESS hresult
/// (0x087A0001), which is why `hr.is_err()` misses it. Not in the vendored
/// bindings, so spelled out here (likewise the two device-lost codes).
const DXGI_STATUS_OCCLUDED: windows_core::HRESULT = windows_core::HRESULT(0x087A0001u32 as i32);
const DXGI_ERROR_DEVICE_REMOVED: windows_core::HRESULT =
    windows_core::HRESULT(0x887A0005u32 as i32);
const DXGI_ERROR_DEVICE_RESET: windows_core::HRESULT = windows_core::HRESULT(0x887A0007u32 as i32);

/// A frame-latency wait that timed out: skip presenting (the old
/// code fired a `Present(1)` + DO_NOT_WAIT into a 33 ms churn instead), but
/// re-probe now and then so a transient DWM stall cannot freeze the window.
const LATENCY_TIMEOUT_PROBE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

/// Fallback display period used until DXGI frame statistics report a real one.
const DEFAULT_REFRESH_PERIOD: f64 = 1.0 / 60.0;

/// `IDXGISwapChain::GetFrameStatistics` has a vtable slot in the vendored
/// bindings but no generated wrapper, so call it through the vtable. This is the
/// only source of a driver-side vblank timestamp (`SyncQPCTime`, in the same QPC
/// domain `Win32Time` counts) plus the refresh counter the period is measured from.
unsafe fn get_frame_statistics(
    swap_chain: &IDXGISwapChain1,
    stats: &mut DXGI_FRAME_STATISTICS,
) -> windows_core::HRESULT {
    let base: &IDXGISwapChain = swap_chain;
    unsafe {
        (Interface::vtable(base).GetFrameStatistics)(Interface::as_raw(base), stats as *mut _)
    }
}

/// `ID3D11Device::GetDeviceRemovedReason` has a vtable slot in the vendored bindings but no
/// generated wrapper, so it is called through the vtable the way `get_frame_statistics` is.
///
/// It is the authoritative answer to "is this device still usable": it latches the real cause
/// and keeps reporting it, and unlike a `Present` HRESULT it is available when the call that
/// failed was a `CreateBuffer` on a window that never got as far as presenting.
unsafe fn device_removed_reason(device: &ID3D11Device) -> windows_core::HRESULT {
    unsafe { (Interface::vtable(device).GetDeviceRemovedReason)(Interface::as_raw(device)) }
}

/// Swap-chain headroom for a vsync-paced main window: 3 buffers with a maximum
/// frame latency of 2 lets the CPU build frame N+1 while the compositor still
/// holds N, which is what keeps a beat-paced loop from stalling on every hitch.
/// `MAKEPAD_WIN_LATENCY=1` restores the previous minimum-latency configuration
/// (2 buffers, latency 1) for latency-sensitive work like drawing/ink.
fn main_window_latency() -> u32 {
    use std::sync::OnceLock;
    static V: OnceLock<u32> = OnceLock::new();
    *V.get_or_init(|| match std::env::var("MAKEPAD_WIN_LATENCY").ok().as_deref() {
        Some("1") => 1,
        _ => 2,
    })
}

/// Buffer count must stay one ahead of the frame latency, and `ResizeBuffers`
/// has to be handed the same count the chain was created with.
fn main_window_buffer_count() -> u32 {
    main_window_latency() + 1
}

pub struct D3d11Window {
    pub window_id: WindowId,
    pub is_in_resize: bool,
    pub window_geom: WindowGeom,
    pub win32_window: Box<Win32Window>,
    pub render_target_view: Option<ID3D11RenderTargetView>,
    pub swap_texture: Option<ID3D11Texture2D>,
    /// Logical inner size the swap-chain buffers were last allocated for.
    /// Compared together with `alloc_dpi`: a DPI-only change (cross-monitor)
    /// keeps logical size stable but still needs ResizeBuffers for physical pixels.
    pub alloc_size: Vec2d,
    /// DPI factor the swap-chain buffers were last allocated for.
    pub alloc_dpi: f64,
    pub first_draw: bool,
    pub swap_chain: IDXGISwapChain1,
    /// The DXGI frame-latency waitable object for this swap chain, used to pace
    /// the CPU render loop to the display refresh (vblank). It is created by
    /// requesting the `FRAME_LATENCY_WAITABLE_OBJECT` swap-chain flag and
    /// retrieving it via `IDXGISwapChain2::GetFrameLatencyWaitableObject`.
    /// For windows that do not use waitable pacing (e.g. popups), this is a
    /// null/invalid `HANDLE` and callers must skip waiting on it.
    /// The handle is owned by the *application* (it is a separate kernel object,
    /// not the swap chain itself), so we close it in `Drop`.
    pub frame_latency_waitable: HANDLE,
    /// Whether this swap chain was created with the `FRAME_LATENCY_WAITABLE_OBJECT`
    /// flag. `ResizeBuffers` must pass the SAME flags the chain was created with, so
    /// we track this at creation rather than inferring it from `frame_latency_waitable`
    /// (which can be null even on a waitable chain if the `IDXGISwapChain2` cast failed).
    pub waitable_swap_chain: bool,
    /// Once-per-failure-episode log latch for `resize_buffers` errors, which are
    /// retried every paint and would otherwise log on each attempt.
    pub resize_error_logged: bool,
    /// Same once-per-failure-episode latch for the `present` error path.
    pub present_error_logged: bool,
    /// Estimated display refresh period in seconds, refined from successive DXGI
    /// frame statistics (`SyncQPCTime` / `SyncRefreshCount` deltas) so the beat's
    /// target-flip timestamp lands on a real vblank rather than a nominal 60 Hz.
    pub refresh_period: f64,
    /// Last frame-statistics sample: (SyncRefreshCount, its app time).
    pub last_frame_stats: Option<(u32, f64)>,
    /// When this window first reported itself occluded or minimized; drives the
    /// probe interval that keeps a stale flag from freezing it forever.
    pub occluded_since: Option<std::time::Instant>,
    /// When its frame-latency wait first timed out; same probe treatment.
    pub latency_timeout_since: Option<std::time::Instant>,
    /// The D3D device was removed/reset. Nothing this window presents can ever
    /// land again, so the paint loop stops re-dirtying its pass (recovery would
    /// mean recreating the device, which is out of scope).
    pub device_lost: bool,
}

impl D3d11Window {
    /// How long a window stays skipped after it reported itself occluded or
    /// minimized before the paint loop spends one frame probing whether that is
    /// still true. Matches the macOS backend's `OCCLUSION_PROBE_INTERVAL`; both
    /// flags can stick on "hidden" while the window is really on screen.
    pub const OCCLUSION_PROBE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

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
        win32_window.set_ime_active(false);
        let wg = win32_window.get_window_geom();

        let sc_desc = DXGI_SWAP_CHAIN_DESC1 {
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            BufferCount: main_window_buffer_count(),
            Width: (wg.inner_size.x * wg.dpi_factor) as u32,
            Height: (wg.inner_size.y * wg.dpi_factor) as u32,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            // Request a frame-latency waitable object so the render loop can pace
            // the CPU to the display refresh (vblank) by waiting on it once per
            // frame, instead of spinning. ResizeBuffers must pass this same flag.
            Flags: DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32,
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

            // Set the maximum frame latency on the *swap chain* (not the device)
            // and retrieve its frame-latency waitable object — the beat the event
            // loop waits on, one credit per retired present. Latency 2 (with 3
            // buffers) gives the CPU a frame of headroom so a single slow tick
            // does not cost a whole refresh; `MAKEPAD_WIN_LATENCY=1` restores the
            // old minimum-latency pair.
            let frame_latency_waitable = match swap_chain.cast::<IDXGISwapChain2>() {
                Ok(swap_chain2) => {
                    let _ = swap_chain2.SetMaximumFrameLatency(main_window_latency());
                    swap_chain2.GetFrameLatencyWaitableObject()
                }
                Err(_) => HANDLE(std::ptr::null_mut()),
            };
            // Publish it as this window's paint beat. Registration order decides
            // the primary window (index 0), whose beat drives the whole app tick.
            if !frame_latency_waitable.is_invalid() {
                with_win32_app(|app| {
                    app.register_beat_handle(window_id, frame_latency_waitable, false)
                });
            }

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
                alloc_dpi: wg.dpi_factor,
                window_geom: wg,
                win32_window: win32_window,
                swap_texture: Some(swap_texture),
                render_target_view: render_target_view,
                swap_chain: swap_chain,
                frame_latency_waitable,
                waitable_swap_chain: true,
                resize_error_logged: false,
                present_error_logged: false,
                refresh_period: DEFAULT_REFRESH_PERIOD,
                last_frame_stats: None,
                occluded_since: None,
                latency_timeout_since: None,
                device_lost: false,
            }
        }
    }

    pub fn new_popup(
        window_id: WindowId,
        d3d11_cx: &D3d11Cx,
        size: Vec2d,
        position: Vec2d,
    ) -> D3d11Window {
        let mut win32_window = Box::new(Win32Window::new_popup(window_id, position, size));
        win32_window.init(size);

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

            // Keep the low (1-frame) latency that the old device-level
            // SetMaximumFrameLatency(1) used to give popups, but WITHOUT requesting
            // the waitable flag (popups are not paced via a waitable object).
            if let Ok(swap_chain2) = swap_chain.cast::<IDXGISwapChain2>() {
                let _ = swap_chain2.SetMaximumFrameLatency(1);
            }

            let swap_texture = swap_chain.GetBuffer(0).unwrap();
            let mut render_target_view = None;
            d3d11_cx
                .device
                .CreateRenderTargetView(&swap_texture, None, Some(&mut render_target_view))
                .unwrap();

            D3d11Window {
                first_draw: true,
                is_in_resize: false,
                window_id,
                alloc_size: wg.inner_size,
                alloc_dpi: wg.dpi_factor,
                window_geom: wg,
                win32_window,
                swap_texture: Some(swap_texture),
                render_target_view,
                swap_chain,
                // Popups are not paced via a waitable object; store a null handle
                // and the render loop will skip waiting on it.
                frame_latency_waitable: HANDLE(std::ptr::null_mut()),
                waitable_swap_chain: false,
                resize_error_logged: false,
                present_error_logged: false,
                refresh_period: DEFAULT_REFRESH_PERIOD,
                last_frame_stats: None,
                occluded_since: None,
                latency_timeout_since: None,
                device_lost: false,
            }
        }
    }

    pub fn start_resize(&mut self) {
        self.is_in_resize = true;
        // A live resize presents unpaced (Present(0) + DwmFlush), so its waitable
        // is no longer a frame clock — retire the beat and let the resize SetTimer
        // heartbeat drive the loop until the drag ends.
        try_with_win32_app(|app| app.unregister_beat_handle(self.window_id));
    }

    // switch back to swapchain
    pub fn stop_resize(&mut self) {
        self.is_in_resize = false;
        // Force ResizeBuffers on the next paint (logical size and/or DPI may have
        // changed while the user was dragging across monitors).
        self.alloc_size = Vec2d::default();
        self.alloc_dpi = 0.0;
        // Live-resize presents without vsync and skips the frame-latency wait, but
        // every retired Present still credits the waitable semaphore, so a credit
        // is almost certainly waiting. Claim it rather than waiting for the next
        // one: the first frame after the drag then goes out immediately instead of
        // sitting out a whole refresh (the old code drained the semaphore to zero
        // here, which made every resize end with a visible stall). Any surplus
        // credits are capped by the chain's maximum frame latency, so pacing
        // re-establishes itself within a frame or two by itself.
        if !self.frame_latency_waitable.is_invalid() {
            let (window_id, handle) = (self.window_id, self.frame_latency_waitable);
            try_with_win32_app(|app| app.register_beat_handle(window_id, handle, true));
        }
    }

    /// The app-time of the first display flip at or after `wake_time` — the
    /// timestamp the whole frame is stamped with, so animation advances by the
    /// interval the frame will actually be *shown* for rather than by whenever
    /// each individual pass happened to sample the wall clock.
    ///
    /// DXGI reports the last vblank it synced to (`SyncQPCTime`, raw QPC — the
    /// same clock `Win32Time` counts) plus a refresh counter; stepping forward in
    /// whole refresh periods from there lands on the next vblank. The period
    /// itself is measured from successive samples, since DXGI does not report it.
    pub fn target_present_time(&mut self, wake_time: f64) -> f64 {
        let mut stats = DXGI_FRAME_STATISTICS::default();
        // DXGI_ERROR_FRAME_STATISTICS_DISJOINT (and a fresh chain that has not
        // presented yet) means the sequence broke: fall back to one period out
        // from the wake time and start the estimate over.
        if unsafe { get_frame_statistics(&self.swap_chain, &mut stats) }.is_err()
            || stats.SyncQPCTime == 0
        {
            self.last_frame_stats = None;
            return wake_time + self.refresh_period;
        }
        let sync_time = with_win32_app(|app| app.time.qpc_to_time(stats.SyncQPCTime));
        if let Some((prev_refresh, prev_time)) = self.last_frame_stats {
            let refreshes = stats.SyncRefreshCount.wrapping_sub(prev_refresh);
            let elapsed = sync_time - prev_time;
            if refreshes > 0 && refreshes < 1000 && elapsed > 0.0 {
                let estimate = elapsed / refreshes as f64;
                // 20 Hz..500 Hz; anything outside that is a bad sample, not a display.
                if estimate > 0.002 && estimate < 0.05 {
                    self.refresh_period = self.refresh_period * 0.75 + estimate * 0.25;
                }
            }
        }
        self.last_frame_stats = Some((stats.SyncRefreshCount, sync_time));
        let period = self.refresh_period;
        let target = if sync_time > wake_time {
            sync_time
        } else {
            let steps = ((wake_time - sync_time) / period).floor() + 1.0;
            sync_time + steps * period
        };
        // Never hand the app a timestamp far in the future if the stats are stale.
        target.min(wake_time + 0.1)
    }

    /// Update the swap chain's background color to match the pass clear
    /// color. With DXGI_SCALING_NONE, any gap between the (old-size) swap
    /// chain buffer and the (new-size) window is filled with this color.
    /// By matching the app's background, the gap becomes invisible.
    pub fn sync_background_color(&self, clear_color: crate::makepad_math::Vec4f) {
        unsafe {
            let _ = self.swap_chain.SetBackgroundColor(&mut DXGI_RGBA {
                r: clear_color.x,
                g: clear_color.y,
                b: clear_color.z,
                a: clear_color.w,
            });
        }
    }

    fn clear_alloc_size(&mut self) {
        self.alloc_size = Vec2d::default();
        self.alloc_dpi = 0.0;
    }

    pub fn resize_buffers(&mut self, d3d11_cx: &D3d11Cx) {
        // Buffers are sized in physical pixels (inner_size * dpi_factor). A
        // cross-monitor move can change DPI while keeping logical size the same;
        // skipping ResizeBuffers then leaves DXGI_SCALING_NONE presenting a
        // mismatched buffer (blank/letterboxed until something else forces resize).
        if self.alloc_size == self.window_geom.inner_size
            && self.alloc_dpi == self.window_geom.dpi_factor
        {
            return;
        }
        let inner = self.window_geom.inner_size;
        let dpi = self.window_geom.dpi_factor;
        if (inner.x * dpi) < 1.0 || (inner.y * dpi) < 1.0 {
            return; // ResizeBuffers rejects zero dimensions.
        }
        self.alloc_size = self.window_geom.inner_size;
        self.alloc_dpi = self.window_geom.dpi_factor;
        // ResizeBuffers requires all references to the old backbuffers released first.
        self.swap_texture = None;
        self.render_target_view = None;

        // Any step below can transiently fail during a display change; each error path
        // logs and resets alloc_size so the next paint retries, instead of crashing.
        unsafe {
            let wg = &self.window_geom;
            // ResizeBuffers must be given the SAME flags the swap chain was
            // created with, or it fails with DXGI_ERROR_INVALID_CALL. We track how
            // the chain was created (`waitable_swap_chain`) rather than inferring it
            // from the handle, which can be null even on a waitable chain if the
            // IDXGISwapChain2 cast failed.
            let resize_flags = if self.waitable_swap_chain {
                DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT
            } else {
                DXGI_SWAP_CHAIN_FLAG(0)
            };
            let mut resize_ok = true;
            if let Err(e) = self.swap_chain.ResizeBuffers(
                // 0 = keep the count the chain was created with. It used to be
                // hardcoded to 2, which silently shrank a 3-buffer main window
                // back to 2 on the first resize and undid the beat's headroom.
                0,
                (wg.inner_size.x * wg.dpi_factor) as u32,
                (wg.inner_size.y * wg.dpi_factor) as u32,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                resize_flags,
            ) {
                if !self.resize_error_logged {
                    self.resize_error_logged = true;
                    crate::error!("IDXGISwapChain::ResizeBuffers failed: {}", e);
                }
                self.clear_alloc_size();
                resize_ok = false;
                // Fall through: re-acquire the old-size backbuffer so we keep presenting.
            }

            let swap_texture: ID3D11Texture2D = match self.swap_chain.GetBuffer(0) {
                Ok(texture) => texture,
                Err(e) => {
                    if !self.resize_error_logged {
                        self.resize_error_logged = true;
                        crate::error!("IDXGISwapChain::GetBuffer failed: {}", e);
                    }
                    self.clear_alloc_size();
                    return;
                }
            };
            let mut render_target_view = None;
            if let Err(e) = d3d11_cx.device.CreateRenderTargetView(
                &swap_texture,
                None,
                Some(&mut render_target_view),
            ) {
                if !self.resize_error_logged {
                    self.resize_error_logged = true;
                    crate::error!("CreateRenderTargetView failed: {}", e);
                }
                self.clear_alloc_size();
                return;
            }

            self.swap_texture = Some(swap_texture);
            self.render_target_view = render_target_view;
            // Only a fully successful resize ends the failure episode; the fall-through
            // path retries and would re-log every retry if it cleared the latch here.
            if resize_ok {
                self.resize_error_logged = false;
            }
        }
    }

    /// Presents the frame just rendered into the backbuffer. Returns whether a frame
    /// was actually handed to the compositor; on `false` the frame was dropped and the
    /// caller must re-mark the pass dirty to schedule a re-present.
    pub fn present(&mut self, vsync: bool, latency_wait_timed_out: bool) -> bool {
        unsafe {
            // During an active window resize, present without the vsync interval and rely on
            // dwm_flush() below for compositor sync: a blocking Present(1) would add up to a refresh
            // interval of latency to every resize frame, making live-resize feel heavier. Steady-
            // state frames still present with vsync.
            let sync_interval = if vsync && !self.is_in_resize { 1 } else { 0 };
            // After a timed-out frame-latency wait, a blocking Present could park the
            // thread until DWM recovers (potentially forever); present non-blocking
            // instead and treat "still busy" as a benign dropped frame.
            let flags = if latency_wait_timed_out {
                DXGI_PRESENT_DO_NOT_WAIT
            } else {
                DXGI_PRESENT(0)
            };
            let hr = self.swap_chain.Present(sync_interval, flags);
            if hr == DXGI_ERROR_WAS_STILL_DRAWING {
                // DO_NOT_WAIT path only: a benign dropped frame; the caller schedules a retry.
                return false;
            }
            if hr == DXGI_STATUS_OCCLUDED {
                // A SUCCESS hresult, so `is_err()` below never sees it — but nothing
                // reached the screen and DWM will stop retiring our presents, which
                // would then time out the frame-latency wait every frame. Report it
                // as not-presented and let the occlusion probe back us off, the same
                // way the macOS backend handles `occlusionState`.
                self.occluded_since.get_or_insert_with(std::time::Instant::now);
                return false;
            }
            if hr == DXGI_ERROR_DEVICE_REMOVED || hr == DXGI_ERROR_DEVICE_RESET {
                // Nothing this swap chain presents can ever land again (GPU reset,
                // driver update, TDR). Say so loudly, once, and stop the paint loop
                // from re-dirtying the pass forever — full device recreation is a
                // separate job.
                if !self.device_lost {
                    self.device_lost = true;
                    crate::error!(
                        "D3D11 DEVICE LOST ({}): the GPU device was removed or reset; \
                         window {:?} will stop updating until the app is restarted",
                        hr,
                        self.window_id
                    );
                }
                return false;
            }
            if hr.is_err() {
                // Transient failures must not hard-abort the app; log once per failure
                // episode and carry on with stale content.
                if !self.present_error_logged {
                    self.present_error_logged = true;
                    crate::error!("IDXGISwapChain::Present failed: {}", hr);
                }
                return false;
            }
            self.present_error_logged = false;
            // A present that actually landed clears the occlusion back-off.
            self.occluded_since = None;

            // During an active window resize, synchronize with the DWM compositor so the
            // freshly-presented frame is composited before the desktop is repainted at the new size.
            if self.is_in_resize {
                dwm_flush();
            }
            true
        }
    }
}

impl Drop for D3d11Window {
    fn drop(&mut self) {
        // Retire this window's paint beat BEFORE closing the handle, or the event
        // loop would wait on a dead handle (WAIT_FAILED) on its very next pass.
        let window_id = self.window_id;
        try_with_win32_app(|app| app.unregister_beat_handle(window_id));
        // The frame-latency waitable is a separate kernel handle owned by the application
        // (GetFrameLatencyWaitableObject hands back a new reference), so close it on window
        // teardown — otherwise each main-window lifecycle leaks a handle. Popups store a null
        // handle and are skipped. The window is moved by value into/out of the window Vec, so
        // this runs exactly once.
        if !self.frame_latency_waitable.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.frame_latency_waitable);
            }
        }
    }
}

#[derive(Clone)]
pub struct D3d11Cx {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    pub query: ID3D11Query,
    pub factory: IDXGIFactory2,
    /// The device has been removed or reset, so every object created from it is dead and the
    /// recovery driver owns the window until it has rebuilt them. A `Cell` because every
    /// resource-creation site in this file holds only a `&D3d11Cx`.
    pub device_lost: Cell<bool>,
    /// Once-per-outage latch for failures the device itself says it survived, so a call site
    /// that runs every frame cannot fill the log.
    other_error_logged: Cell<bool>,
}

impl D3d11Cx {
    /// Builds the device tier: factory, adapter, device, immediate context and event query.
    ///
    /// Fallible because recovery calls it while the display driver may still be restarting,
    /// when `EnumAdapters` and `D3D11CreateDevice` fail transiently for a few hundred
    /// milliseconds. Every argument is a literal, so nothing here depends on retained state.
    fn create_device_tier(
    ) -> windows_core::Result<(IDXGIFactory2, ID3D11Device, ID3D11DeviceContext, ID3D11Query)> {
        unsafe {
            // A DXGI factory snapshots its adapter enumeration when it is created, so one made
            // before a hybrid-GPU transition or a driver reinstall keeps handing back the
            // adapter that went away. Recovery always starts from a fresh factory.
            let factory: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
            let adapter = factory.EnumAdapters(0)?;
            let mut device: Option<ID3D11Device> = None;
            let mut context: Option<ID3D11DeviceContext> = None;
            let mut query: Option<ID3D11Query> = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE(std::ptr::null_mut()),
                D3D11_CREATE_DEVICE_FLAG(0x800 | 0x20), // VIDEO_SUPPORT | BGRA_SUPPORT
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;

            let device = device.unwrap();
            let context = context.unwrap();

            // NOTE: DXGI frame latency is now controlled per-swap-chain via
            // IDXGISwapChain2::SetMaximumFrameLatency(1) in D3d11Window::new,
            // paired with the frame-latency waitable object used to pace the
            // render loop. The old device-level IDXGIDevice1::SetMaximumFrameLatency
            // call has been removed so the two mechanisms don't conflict.

            device.CreateQuery(
                &D3D11_QUERY_DESC {
                    Query: D3D11_QUERY_EVENT,
                    MiscFlags: 0,
                },
                Some(&mut query),
            )?;

            Ok((factory, device, context, query.unwrap()))
        }
    }

    pub fn new() -> D3d11Cx {
        let (factory, device, context, query) =
            Self::create_device_tier().expect("D3D11: could not create the initial device");
        D3d11Cx {
            device,
            context,
            factory,
            query,
            device_lost: Cell::new(false),
            other_error_logged: Cell::new(false),
        }
    }

    /// Replaces the four COM handles with a freshly created device tier, leaving the rest of
    /// the struct alone. `false` means the driver is not ready yet and the caller should retry
    /// on a later tick.
    pub fn recreate_device(&mut self) -> bool {
        match Self::create_device_tier() {
            Ok((factory, device, context, query)) => {
                self.factory = factory;
                self.device = device;
                self.context = context;
                self.query = query;
                self.other_error_logged.set(false);
                true
            }
            Err(e) => {
                crate::error!("D3D11 device recreation failed, will retry: {}", e);
                false
            }
        }
    }

    /// Whether the current device is still usable, asked of the device itself.
    pub fn device_is_alive(&self) -> bool {
        unsafe { device_removed_reason(&self.device).is_ok() }
    }

    /// Where every fallible D3D11 call in this file reports its failure.
    ///
    /// The HRESULT a creation call returns is not always one of the two DXGI device-lost
    /// codes, so the device is asked directly instead of the error being pattern-matched.
    /// Nothing on the healthy path reaches this.
    pub fn note_error(&self, what: &str, err: &windows_core::Error) {
        if unsafe { device_removed_reason(&self.device) }.is_err() {
            if !self.device_lost.replace(true) {
                crate::error!(
                    "D3D11 DEVICE LOST: {} failed ({}). Rebuilding the device and every GPU                      resource; the window will keep its last frame until that succeeds.",
                    what,
                    err
                );
            }
        } else if !self.other_error_logged.replace(true) {
            crate::error!("D3D11 {} failed, device still alive: {}", what, err);
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
        if hresult.is_err() {
            // A removed device fails `GetData` rather than answering it, and `!= S_FALSE` would
            // read that as "the GPU finished this frame" forever — the only device-loss signal
            // the studio-hosted path has, since it renders to a texture and never presents.
            self.note_error("ID3D11DeviceContext::GetData", &windows_core::Error::from(hresult));
            return true;
        }
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
pub struct CxOsUniformBuffer {
    pub buffer: D3d11Buffer,
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
        len_slots: usize,
        data: *const std::ffi::c_void,
    ) {
        // Keep original churn behavior (replace when size changes), but avoid
        // leaking the old COM buffer by creating into a temporary out variable.
        if self.buffer.is_none() || self.last_size != len_slots {
            let mut exact_desc = *buffer_desc;
            exact_desc.ByteWidth = (len_slots * 4) as u32;
            let mut new_buffer = None;
            if let Err(e) = unsafe {
                d3d11_cx
                    .device
                    .CreateBuffer(&exact_desc, None, Some(&mut new_buffer))
            } {
                // Draw-list and pass uniforms come through here every frame with no dirty
                // gate, so a device that died between frames is seen here first — many times
                // over, before any window reaches `present`. Leave the slot empty and zero the
                // size memo, which would otherwise hand the dead buffer straight back, so the
                // ordinary path rebuilds it once there is a live device again.
                d3d11_cx.note_error("ID3D11Device::CreateBuffer", &e);
                self.buffer = None;
                self.last_size = 0;
                return;
            }
            self.last_size = len_slots;
            self.buffer = new_buffer;
        }

        let Some(buffer) = self.buffer.clone() else {
            return;
        };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        let p_mapped: *mut _ = &mut mapped;
        unsafe {
            if let Err(e) = d3d11_cx
                .context
                .Map(&buffer, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(p_mapped))
            {
                // Nothing was mapped, so there is no `Unmap` to pair on this path.
                d3d11_cx.note_error("ID3D11DeviceContext::Map", &e);
                return;
            }
            std::ptr::copy_nonoverlapping(data, mapped.pData, len_slots * 4);
            d3d11_cx.context.Unmap(&buffer, 0);
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

        self.create_buffer_or_update(d3d11_cx, &buffer_desc, len_slots, data);
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
        self.create_buffer_or_update(d3d11_cx, &buffer_desc, len_slots, data);
    }

    pub fn update_with_constant_bytes(&mut self, d3d11_cx: &D3d11Cx, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        let padded_len = data.len().next_multiple_of(16);
        let mut padded = Vec::with_capacity(padded_len);
        padded.extend_from_slice(data);
        padded.resize(padded_len, 0);
        let len_slots = padded.len() >> 2;

        let buffer_desc = D3D11_BUFFER_DESC {
            Usage: D3D11_USAGE_DYNAMIC,
            ByteWidth: padded.len() as u32,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
            MiscFlags: 0,
            StructureByteStride: 0,
        };
        self.create_buffer_or_update(
            d3d11_cx,
            &buffer_desc,
            len_slots,
            padded.as_ptr() as *const _,
        );
    }
}

#[derive(Default)]
pub struct CxOsTexture {
    pub(crate) texture: Option<ID3D11Texture2D>,
    pub shared_handle: HANDLE,
    /// Keyed mutex for cross-process shared textures (studio RunView). Present only
    /// on `SharedBGRAu8` textures created with `D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX`.
    /// The producer (hosted app) brackets its render with Acquire/Release; the consumer
    /// (studio host) takes a transient Acquire/Release to make the writes visible on its
    /// own device — legacy `D3D11_RESOURCE_MISC_SHARED` gives no cross-device coherence.
    pub(crate) keyed_mutex: Option<IDXGIKeyedMutex>,
    pub(crate) shader_resource_view: Option<ID3D11ShaderResourceView>,
    render_target_view: Option<ID3D11RenderTargetView>,
    render_target_face_views: [Option<ID3D11RenderTargetView>; 6],
    depth_stencil_view: Option<ID3D11DepthStencilView>,
    /// Allocated dimensions + DXGI format of `texture` for `Vec*` textures, so that growth-by-append
    /// (the SLUG glyph atlases) can be uploaded into the existing texture via `UpdateSubresource`
    /// instead of recreating it. `vec_alloc_dxgi == 0` (DXGI_FORMAT_UNKNOWN) means "not allocated".
    vec_alloc_width: usize,
    vec_alloc_height: usize,
    vec_alloc_dxgi: i32,
    /// Number of rows already uploaded into `texture`. In-place `UpdateSubresource` reuse is only
    /// allowed for pure appends (rows at/after this), never overwriting rows an in-flight frame
    /// may still be sampling — overwriting them can tear the SDF data and hang the GPU.
    vec_uploaded_height: usize,
}

impl CxTexture {
    pub fn update_vec_texture(&mut self, d3d11_cx: &D3d11Cx) {
        // TODO maybe we can update the data instead of making a new texture?
        if self.alloc_vec() {}
        let updated = self.take_updated();
        if !updated.is_empty() {
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
                            unsafe {
                                data.as_ptr().add(face.saturating_mul(face_pixels)) as *const _
                            }
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

            // Resolve the pixel format, dimensions, bytes-per-pixel and source data pointer for the
            // general (non-cube) Vec* texture formats.
            let (dxgi_format, width, height, bpp, data_ptr): (DXGI_FORMAT, usize, usize, usize, *const u8) =
                match &self.format {
                    TextureFormat::VecBGRAu8_32 { width, height, data, .. } => (
                        DXGI_FORMAT_B8G8R8A8_UNORM, *width, *height, 4,
                        data.as_ref().map_or(std::ptr::null(), |d| d.as_ptr() as *const u8),
                    ),
                    TextureFormat::VecRGBAf32 { width, height, data, .. } => (
                        DXGI_FORMAT_R32G32B32A32_FLOAT, *width, *height, 16,
                        data.as_ref().map_or(std::ptr::null(), |d| d.as_ptr() as *const u8),
                    ),
                    TextureFormat::VecRu8 { width, height, data, .. } => (
                        DXGI_FORMAT_R8_UNORM, *width, *height, 1,
                        data.as_ref().map_or(std::ptr::null(), |d| d.as_ptr() as *const u8),
                    ),
                    TextureFormat::VecRGu8 { width, height, data, .. } => (
                        DXGI_FORMAT_R8G8_UNORM, *width, *height, 2,
                        data.as_ref().map_or(std::ptr::null(), |d| d.as_ptr() as *const u8),
                    ),
                    TextureFormat::VecRf32 { width, height, data, .. } => (
                        DXGI_FORMAT_R32_FLOAT, *width, *height, 4,
                        data.as_ref().map_or(std::ptr::null(), |d| d.as_ptr() as *const u8),
                    ),
                    // Mipmapped images: upload level 0 only for now (safe). Real per-level mip
                    // upload is a TODO before MAKEPAD_IMAGE_MIPMAPS helps on D3D11.
                    TextureFormat::VecMipBGRAu8_32 { width, height, data, .. } => (
                        DXGI_FORMAT_B8G8R8A8_UNORM, *width, *height, 4,
                        data.as_ref().map_or(std::ptr::null(), |d| d.as_ptr() as *const u8),
                    ),
                    _ => panic!(),
                };

            if width == 0 || height == 0 || data_ptr.is_null() {
                // The pixel buffer is out on loan: `Texture::take_vec_*` leaves `data` as
                // `None` until the matching `put_back_*`, which is the glyph atlas's normal
                // state for a whole frame whenever `Fonts::prepare_textures` takes an early
                // return. `take_updated` above already consumed the dirty flag, so re-arm it —
                // dropping it here would mean this texture is never uploaded again and all
                // text disappears permanently. The Metal backend guards the same case.
                self.set_updated(updated);
                return;
            }
            let row_pitch = (width * bpp) as u32;

            // Dirty sub-rect to upload (whole logical region for a Full update).
            let (bx, by, bw, bh) = match updated {
                TextureUpdated::Partial(r) => {
                    let bx = r.origin.x.min(width);
                    let by = r.origin.y.min(height);
                    (bx, by, r.size.width.min(width - bx), r.size.height.min(height - by))
                }
                _ => (0, 0, width, height),
            };

            // Update the existing GPU texture in place via `UpdateSubresource` instead of
            // recreating the whole (up to ~16 MB) texture + SRV — the latter was a per-change hitch
            // on D3D11 during scrolling (GL `glTexSubImage2D` / Metal `replaceRegion` already update
            // in place). `UpdateSubresource` is serialized by the runtime, so a sub-rect update
            // never tears a normally-sampled texture.
            //
            // The one exception is the SDF/slug glyph atlas (VecRGBAf32): its shader walks a
            // per-glyph curve list whose COUNT comes from the texels, so a mid-frame overwrite of
            // rows an in-flight frame is still sampling can feed garbage counts and hang the GPU
            // (a multi-second TDR). For it, only reuse pure appends (new rows never sampled yet);
            // rebuilds/resets recreate a fresh texture. Bitmap atlases (the color-glyph/emoji atlas,
            // images — sampled by normalized UV) are safe to update in place anywhere.
            let is_sdf = matches!(dxgi_format, DXGI_FORMAT_R32G32B32A32_FLOAT);
            let safe_to_reuse = matches!(updated, TextureUpdated::Partial(_))
                && (!is_sdf || by + 1 >= self.os.vec_uploaded_height);
            let can_reuse = self.os.texture.is_some()
                && self.os.vec_alloc_width == width
                && self.os.vec_alloc_dxgi == dxgi_format.0
                && self.os.vec_alloc_height >= height
                && safe_to_reuse;

            if can_reuse {
                if bw != 0 && bh != 0 {
                    let dst_box = D3D11_BOX {
                        left: bx as u32, top: by as u32, front: 0,
                        right: (bx + bw) as u32, bottom: (by + bh) as u32, back: 1,
                    };
                    let src = unsafe { data_ptr.add((by * width + bx) * bpp) } as *const std::ffi::c_void;
                    let resource: ID3D11Resource = self.os.texture.as_ref().unwrap().cast().unwrap();
                    unsafe {
                        d3d11_cx.context.UpdateSubresource(
                            &resource,
                            0,
                            Some(&dst_box as *const _),
                            src,
                            row_pitch,
                            0,
                        );
                    }
                    self.os.vec_uploaded_height = (by + bh).max(self.os.vec_uploaded_height);
                }
                return;
            }

            // (Re)allocate. For the append-only glyph atlases (RGBAf32), add ~1.5x height headroom
            // (rounded up) so subsequent growth reuses the texture instead of recreating it. Other
            // formats (images/data) are sampled by normalized UV, so they MUST be exact-sized.
            let cap_height = if matches!(dxgi_format, DXGI_FORMAT_R32G32B32A32_FLOAT) {
                // Generous headroom for the append-only glyph atlas: 3x the needed height with a
                // sizable minimum, rounded up. This makes the texture large enough to hold a
                // typical room's full glyph set after the first allocation, so growth-driven
                // recreations (each a full texture realloc) become rare during long flick-scrolls.
                let want = (height * 3).max(512);
                ((want + 127) / 128) * 128
            } else {
                height
            };
            let texture_desc = D3D11_TEXTURE2D_DESC {
                Width: width as u32,
                Height: cap_height as u32,
                MipLevels: 1,
                ArraySize: 1,
                Format: dxgi_format,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let mut texture = None;
            if let Err(e) = unsafe {
                d3d11_cx
                    .device
                    .CreateTexture2D(&texture_desc, None, Some(&mut texture))
            } {
                // Re-arm the update rather than dropping it: this texture is an atlas or an
                // image whose pixels are still in memory, so the next frame against a live
                // device uploads it in full.
                d3d11_cx.note_error("CreateTexture2D(vec)", &e);
                self.set_updated(TextureUpdated::Full);
                return;
            }
            let Some(resource) = texture
                .as_ref()
                .and_then(|t: &ID3D11Texture2D| t.cast::<ID3D11Resource>().ok())
            else {
                self.set_updated(TextureUpdated::Full);
                return;
            };
            // Upload the logical rows (0..height) into the freshly-allocated (possibly taller)
            // texture. Rows height..cap_height stay unused — the glyph shader addresses by absolute
            // texel index, so the extra capacity is never sampled.
            let dst_box = D3D11_BOX {
                left: 0, top: 0, front: 0, right: width as u32, bottom: height as u32, back: 1,
            };
            unsafe {
                d3d11_cx.context.UpdateSubresource(
                    &resource,
                    0,
                    Some(&dst_box as *const _),
                    data_ptr as *const _,
                    row_pitch,
                    0,
                );
            }
            let mut shader_resource_view = None;
            if let Err(e) = unsafe {
                d3d11_cx
                    .device
                    .CreateShaderResourceView(&resource, None, Some(&mut shader_resource_view))
            } {
                // Publishing the texture without its view would leave the alloc bookkeeping
                // claiming a usable texture that nothing can sample.
                d3d11_cx.note_error("CreateShaderResourceView(vec)", &e);
                self.set_updated(TextureUpdated::Full);
                return;
            }
            self.os.texture = texture;
            self.os.shader_resource_view = shader_resource_view;
            self.os.vec_alloc_width = width;
            self.os.vec_alloc_height = cap_height;
            self.os.vec_alloc_dxgi = dxgi_format.0;
            // The full logical data (rows 0..height) was just uploaded into the fresh texture.
            self.os.vec_uploaded_height = height;
        }
    }

    pub fn update_render_target(&mut self, d3d11_cx: &D3d11Cx, width: usize, height: usize) {
        if self.alloc_render(width, height) {
            let alloc = self.alloc.as_ref().unwrap();
            let is_cube = matches!(&self.format, TextureFormat::RenderCubeBGRAu8 { .. });
            let misc_flags = if is_cube {
                D3D11_RESOURCE_MISC_TEXTURECUBE
            } else {
                D3D11_RESOURCE_MISC_FLAG(0)
            };
            let format = texture_pixel_to_dx11_pixel(&alloc.pixel);

            let texture_desc = D3D11_TEXTURE2D_DESC {
                Width: width as u32,
                Height: height as u32,
                MipLevels: 1,
                ArraySize: if is_cube { 6 } else { 1 },
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
            if let Err(e) = unsafe {
                d3d11_cx
                    .device
                    .CreateTexture2D(&texture_desc, None, Some(&mut texture))
            } {
                // A render target has no CPU-side contents to preserve, so there is nothing to
                // re-arm: clearing the alloc record is what makes the next pass rebuild it.
                d3d11_cx.note_error("CreateTexture2D(render target)", &e);
                self.alloc = None;
                return;
            }
            let Some(resource) = texture
                .as_ref()
                .and_then(|t: &ID3D11Texture2D| t.cast::<ID3D11Resource>().ok())
            else {
                self.alloc = None;
                return;
            };
            let mut shader_resource_view = None;
            unsafe {
                if is_cube {
                    let srv_desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
                        Format: format,
                        ViewDimension: D3D_SRV_DIMENSION_TEXTURECUBE,
                        Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                            TextureCube: D3D11_TEXCUBE_SRV {
                                MostDetailedMip: 0,
                                MipLevels: 1,
                            },
                        },
                    };
                    d3d11_cx.device.CreateShaderResourceView(
                        &resource,
                        Some(&srv_desc),
                        Some(&mut shader_resource_view),
                    )
                } else {
                    d3d11_cx.device.CreateShaderResourceView(
                        &resource,
                        None,
                        Some(&mut shader_resource_view),
                    )
                }
                .unwrap()
            };
            let mut render_target_view = None;
            let mut render_target_face_views: [Option<ID3D11RenderTargetView>; 6] =
                Default::default();
            if is_cube {
                for face in 0..6u32 {
                    let rtv_desc = D3D11_RENDER_TARGET_VIEW_DESC {
                        Format: format,
                        ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2DARRAY,
                        Anonymous: D3D11_RENDER_TARGET_VIEW_DESC_0 {
                            Texture2DArray: D3D11_TEX2D_ARRAY_RTV {
                                MipSlice: 0,
                                FirstArraySlice: face,
                                ArraySize: 1,
                            },
                        },
                    };
                    unsafe {
                        d3d11_cx.device.CreateRenderTargetView(
                            &resource,
                            Some(&rtv_desc),
                            Some(&mut render_target_face_views[face as usize]),
                        )
                    }
                    .unwrap();
                }
            } else if let Err(e) = unsafe {
                d3d11_cx
                    .device
                    .CreateRenderTargetView(&resource, None, Some(&mut render_target_view))
            } {
                d3d11_cx.note_error("CreateRenderTargetView(render target)", &e);
                self.alloc = None;
                return;
            }

            self.os.texture = texture;
            self.os.shader_resource_view = shader_resource_view;
            self.os.render_target_view = render_target_view;
            self.os.render_target_face_views = render_target_face_views;
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
                // Legacy plain D3D11_RESOURCE_MISC_SHARED (0x2) gives NO cross-device
                // coherence between the two process-local D3D11 devices, so the studio host
                // sampled the hosted app's writes as black. Use a keyed mutex
                // (D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX = 0x100) — which both serializes
                // access and flushes writes across devices — shared via an NT handle
                // (D3D11_RESOURCE_MISC_SHARED_NTHANDLE = 0x800). Keyed-mutex resources are
                // incompatible with the legacy GetSharedHandle/OpenSharedResource path.
                MiscFlags: 0x100 | 0x800,
            };

            let mut texture = None;
            let create_res = unsafe {
                d3d11_device.CreateTexture2D(&texture_desc, None, Some(&mut texture))
            };
            if let Err(err) = &create_res {
                crate::error!("WINHOST: CreateTexture2D(shared keyed-mutex) FAILED: {:?} miscflags={:x}", err, texture_desc.MiscFlags);
                return;
            }
            let resource: ID3D11Resource = texture.clone().unwrap().cast().unwrap();
            let mut shader_resource_view = None;
            unsafe {
                d3d11_device
                    .CreateShaderResourceView(&resource, None, Some(&mut shader_resource_view))
                    .unwrap()
            };

            // NT handles are process-local, so instead of passing the raw value we register
            // a name derived from the presentable-image id (which the hosted app also has),
            // and it opens the resource by that same name — no cross-process handle
            // duplication needed. The returned NT handle is kept open in `shared_handle` to
            // keep the named object alive for the app to find.
            let id_u64 = match &self.format {
                TextureFormat::SharedBGRAu8 { id, .. } => id.as_u64(),
                _ => 0,
            };
            let mut name_wide: Vec<u16> = shared_texture_name(id_u64).encode_utf16().collect();
            name_wide.push(0);
            let mut handle = HANDLE(std::ptr::null_mut());
            match resource.cast::<IDXGIResource1>() {
                Ok(dxgi_resource1) => {
                    let hr = unsafe {
                        (Interface::vtable(&dxgi_resource1).CreateSharedHandle)(
                            Interface::as_raw(&dxgi_resource1),
                            std::ptr::null(),
                            DXGI_SHARED_RESOURCE_READ | DXGI_SHARED_RESOURCE_WRITE,
                            PCWSTR(name_wide.as_ptr()),
                            &mut handle,
                        )
                    };
                    crate::log!("WINHOST: CreateSharedHandle hr={:?} handle={:?} size={}x{} id={:x}", hr, handle.0, alloc.width, alloc.height, id_u64);
                }
                Err(err) => {
                    crate::error!("WINHOST: IDXGIResource1 cast failed: {:?}", err);
                }
            }
            let keyed_mutex: Option<IDXGIKeyedMutex> = resource.cast().ok();
            crate::log!("WINHOST: update_shared_texture keyed_mutex={}", keyed_mutex.is_some());

            self.os.texture = texture;
            self.os.shader_resource_view = shader_resource_view;
            self.os.shared_handle = handle;
            self.os.keyed_mutex = keyed_mutex;
        }
    }

    /// Open the cross-process shared texture the studio host created, by the name derived
    /// from the presentable-image id (see `update_shared_texture`). `_handle` is the legacy
    /// value from the protocol and is unused now that keyed-mutex/NT-handle sharing is
    /// name-based.
    pub fn update_from_shared_handle(&mut self, d3d11_cx: &D3d11Cx, _handle: HANDLE) {
        let did_alloc = self.alloc_shared();
        if !did_alloc {
            return;
        }
        let id_u64 = match &self.format {
            TextureFormat::SharedBGRAu8 { id, .. } => id.as_u64(),
            _ => 0,
        };
        let device1: ID3D11Device1 = match d3d11_cx.device.cast() {
            Ok(d) => d,
            Err(err) => {
                crate::error!("WINCHILD: ID3D11Device1 cast failed: {:?}", err);
                return;
            }
        };
        let mut name_wide: Vec<u16> = shared_texture_name(id_u64).encode_utf16().collect();
        name_wide.push(0);
        let mut resource_ptr: *mut core::ffi::c_void = std::ptr::null_mut();
        let hr = unsafe {
            (Interface::vtable(&device1).OpenSharedResourceByName)(
                Interface::as_raw(&device1),
                PCWSTR(name_wide.as_ptr()),
                DXGI_SHARED_RESOURCE_READ | DXGI_SHARED_RESOURCE_WRITE,
                &<ID3D11Texture2D as Interface>::IID,
                &mut resource_ptr,
            )
        };
        if hr.is_err() || resource_ptr.is_null() {
            crate::error!("WINCHILD: OpenSharedResourceByName FAILED hr={:?} id={:x}", hr, id_u64);
            return;
        }
        let texture: ID3D11Texture2D = unsafe { ID3D11Texture2D::from_raw(resource_ptr) };
        let resource: ID3D11Resource = texture.clone().cast().unwrap();
        let mut shader_resource_view = None;
        let srv = unsafe {
            d3d11_cx
                .device
                .CreateShaderResourceView(&resource, None, Some(&mut shader_resource_view))
        };
        let mut render_target_view = None;
        let rtv = unsafe {
            d3d11_cx
                .device
                .CreateRenderTargetView(&resource, None, Some(&mut render_target_view))
        };
        let keyed_mutex: Option<IDXGIKeyedMutex> = resource.cast().ok();
        crate::log!("WINCHILD: OpenSharedResourceByName OK SRV={:?} RTV={:?} keyed_mutex={} id={:x}", srv, rtv, keyed_mutex.is_some(), id_u64);
        self.os.texture = Some(texture);
        self.os.render_target_view = render_target_view;
        self.os.shader_resource_view = shader_resource_view;
        self.os.keyed_mutex = keyed_mutex;
    }
}

/// Session-local name of the cross-process RunView shared texture, derived from the
/// presentable-image id so the studio host and the hosted app agree without extra plumbing.
fn shared_texture_name(id_u64: u64) -> String {
    format!("Local\\makepad-runview-{:016x}", id_u64)
}

/// DXGI shared-resource access rights for CreateSharedHandle / OpenSharedResourceByName.
const DXGI_SHARED_RESOURCE_READ: u32 = 0x8000_0000;
const DXGI_SHARED_RESOURCE_WRITE: u32 = 0x0000_0001;

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
            let mut blend_state = None;
            unsafe {
                d3d11_cx
                    .device
                    .CreateBlendState(&blend_desc, Some(&mut blend_state))
                    .unwrap()
            }
            self.blend_state = blend_state;
        }

        if self.raster_state_no_cull.is_none() || self.raster_state_backface_cull.is_none() {
            let make_raster_state = |cull_mode| {
                let raster_desc = D3D11_RASTERIZER_DESC {
                    AntialiasedLineEnable: FALSE,
                    CullMode: cull_mode,
                    DepthBias: 0,
                    DepthBiasClamp: 0.0,
                    DepthClipEnable: TRUE,
                    FillMode: D3D11_FILL_SOLID,
                    FrontCounterClockwise: FALSE,
                    MultisampleEnable: FALSE,
                    ScissorEnable: FALSE,
                    SlopeScaledDepthBias: 0.0,
                };
                let mut raster_state = None;
                unsafe {
                    d3d11_cx
                        .device
                        .CreateRasterizerState(&raster_desc, Some(&mut raster_state))
                        .unwrap()
                }
                raster_state
            };
            self.raster_state_no_cull = make_raster_state(D3D11_CULL_NONE);
            self.raster_state_backface_cull = make_raster_state(D3D11_CULL_BACK);
        }

        if self.depth_stencil_state_write.is_none() {
            let make_depth_stencil_state = |depth_write_mask| {
                let ds_desc = D3D11_DEPTH_STENCIL_DESC {
                    DepthEnable: TRUE,
                    DepthWriteMask: depth_write_mask,
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
                let mut depth_stencil_state = None;
                unsafe {
                    d3d11_cx
                        .device
                        .CreateDepthStencilState(&ds_desc, Some(&mut depth_stencil_state))
                        .unwrap()
                }
                depth_stencil_state
            };
            self.depth_stencil_state_write = make_depth_stencil_state(D3D11_DEPTH_WRITE_MASK_ALL);
            self.depth_stencil_state_no_write =
                make_depth_stencil_state(D3D11_DEPTH_WRITE_MASK_ZERO);
        }

        unsafe {
            d3d11_cx
                .context
                .RSSetState(self.raster_state_no_cull.as_ref().unwrap());
            let blend_factor = [0., 0., 0., 0.];
            d3d11_cx.context.OMSetBlendState(
                self.blend_state.as_ref().unwrap(),
                Some(&blend_factor),
                0xffffffff,
            );
            if let Some(depth_stencil_state) = self.depth_stencil_state_write.as_ref() {
                d3d11_cx
                    .context
                    .OMSetDepthStencilState(depth_stencil_state, 0);
            }
        }
    }
}

#[derive(Default, Clone)]
pub struct CxOsPass {
    pass_uniforms: D3d11Buffer,
    blend_state: Option<ID3D11BlendState>,
    raster_state_no_cull: Option<ID3D11RasterizerState>,
    raster_state_backface_cull: Option<ID3D11RasterizerState>,
    depth_stencil_state_write: Option<ID3D11DepthStencilState>,
    depth_stencil_state_no_write: Option<ID3D11DepthStencilState>,
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
            output.use_vulkan = false;

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

fn shader_cache_dir() -> Option<&'static std::path::Path> {
    use std::sync::OnceLock;

    static DIR: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let path_ptr =
            unsafe { SHGetKnownFolderPath(&FOLDERID_LocalAppData, KF_FLAG_DEFAULT, None) }.ok()?;
        let path_str = unsafe { path_ptr.to_string().ok() };
        unsafe { CoTaskMemFree(Some(path_ptr.as_ptr() as _)) };
        let path = std::path::PathBuf::from(path_str?)
            .join("makepad")
            .join("d3d11_shader_cache");
        std::fs::create_dir_all(&path).ok()?;
        Some(path)
    })
    .as_deref()
}

// FNV-1a 64-bit hash of the HLSL source — used as the on-disk cache key.
// The leading seed byte lets us invalidate every cache entry by bumping
// CACHE_KEY_VERSION whenever the compile flags or entry points change, which
// would otherwise leave stale bytecode on disk that no longer matches what
// the runtime expects.
fn hlsl_cache_key(hlsl: &str) -> u64 {
    const CACHE_KEY_VERSION: u8 = 2;
    let mut hash: u64 = 0xcbf29ce484222325;
    hash ^= CACHE_KEY_VERSION as u64;
    hash = hash.wrapping_mul(0x100000001b3);
    for byte in hlsl.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

// Invoke D3DCompile (fxcompiler) on one stage. Thread-safe (pure CPU work)
// so can be called from a background thread to parallelize startup compile.
//
// We pass D3DCOMPILE_SKIP_OPTIMIZATION because FXC's optimizer is what makes
// shader compile times explode — it can spend many seconds on a single text
// shader with loops. UI shaders are short-lived per frame and the win from
// FXC-level optimization is tiny for this workload, while the cold-cache
// startup cost is huge. If a specific shader is later shown to be a runtime
// hotspot, it should be recompiled with optimizations on a background thread
// and hot-swapped — that's a cleaner solution than paying the cost upfront
// for every shader in the app.
fn d3d_compile_hlsl(target: &str, entry: &str, shader: &str) -> Result<Vec<u8>, String> {
    const D3DCOMPILE_SKIP_OPTIMIZATION: u32 = 1 << 2;
    const D3DCOMPILE_ENABLE_BACKWARDS_COMPATIBILITY: u32 = 1 << 12;
    const FLAGS: u32 = D3DCOMPILE_SKIP_OPTIMIZATION | D3DCOMPILE_ENABLE_BACKWARDS_COMPATIBILITY;
    unsafe {
        let shader_bytes = shader.as_bytes();
        let mut blob = None;
        let mut errors = None;
        if D3DCompile(
            shader_bytes.as_ptr() as *const _,
            shader_bytes.len(),
            PCSTR("makepad_shader\0".as_ptr()),
            None,
            None,
            PCSTR(entry.as_ptr()),
            PCSTR(target.as_ptr()),
            FLAGS,
            0,
            &mut blob,
            Some(&mut errors),
        )
        .is_ok()
        {
            let blob = blob.unwrap();
            let ptr = blob.GetBufferPointer() as *const u8;
            let len = blob.GetBufferSize();
            return Ok(std::slice::from_raw_parts(ptr, len).to_vec());
        }
        let error = errors.unwrap();
        let pointer = error.GetBufferPointer();
        let size = error.GetBufferSize();
        let slice = std::slice::from_raw_parts(pointer as *const u8, size as usize);
        Err(String::from_utf8_lossy(slice).into_owned())
    }
}

// Cheap existence check used to decide whether a shader can go through the
// synchronous fast path (disk reads only) or needs the async background
// compile path. We only check existence, not contents — if the files are
// present but corrupt/short, the subsequent read path will catch that.
fn shader_bytes_cached(cache_dir: Option<&std::path::Path>, cache_key: u64) -> bool {
    let Some(dir) = cache_dir else {
        return false;
    };
    let vs = dir.join(format!("{:016x}_vs.dxbc", cache_key));
    let ps = dir.join(format!("{:016x}_ps.dxbc", cache_key));
    vs.exists() && ps.exists()
}

// Read the DXBC blob from the on-disk cache if present, otherwise compile and
// write it. Disk I/O and D3DCompile are both thread-safe so this can run on a
// worker thread.
fn get_or_compile_shader_bytes(
    cache_dir: Option<&std::path::Path>,
    cache_key: u64,
    suffix: &str,
    target: &str,
    entry: &str,
    hlsl: &str,
) -> Result<Vec<u8>, String> {
    if let Some(dir) = cache_dir {
        let path = dir.join(format!("{:016x}{}.dxbc", cache_key, suffix));
        if let Ok(bytes) = std::fs::read(&path) {
            return Ok(bytes);
        }
        let bytes = d3d_compile_hlsl(target, entry, hlsl)?;
        let _ = std::fs::write(&path, &bytes);
        return Ok(bytes);
    }
    d3d_compile_hlsl(target, entry, hlsl)
}

/// Result of a background D3DCompile for one shader.
///
/// The worker writes the compiled bytes to the on-disk cache before sending
/// this result, so the main thread picks them back up via the disk cache in
/// `CxOsDrawShader::new`. We only carry status (not the bytes themselves) so
/// the channel doesn't ferry hundreds of KB of DXBC — the SLUG helper alone
/// is ~240 KB. Error strings are kept for diagnostic output when a compile
/// fails.
struct AsyncCompileResult {
    shader_id: usize,
    vs_status: Result<(), String>,
    ps_status: Result<(), String>,
}

/// Background HLSL compile queue used for `async_compile: true` shaders.
///
/// The DrawTextSlug helper is by far the most expensive shader to compile on
/// Windows (hundreds of KB of DXBC, multiple seconds with the default FXC
/// settings) and it is the primary motivation for this path — without it the
/// SLUG helper blocks the main thread the first time a SLUG glyph is needed.
/// Other shaders stay on the synchronous parallel-precompile path so the app
/// still renders its widgets immediately on the first frame.
///
/// The worker threads call `D3DCompile`, write the resulting bytecode into
/// the on-disk shader cache, then send a lightweight result to the main
/// thread via an mpsc channel. The main thread drains completed results
/// each paint tick, creates the D3D11 shader objects, and requests a redraw
/// so the now-ready widgets get a chance to render.
pub struct AsyncHlslCompile {
    inner: std::sync::Mutex<AsyncHlslCompileInner>,
}

struct AsyncHlslCompileInner {
    tx: std::sync::mpsc::Sender<AsyncCompileResult>,
    rx: std::sync::mpsc::Receiver<AsyncCompileResult>,
    pending: std::collections::HashSet<usize>,
    /// Finished compiles that have been received from workers but whose D3D11 objects haven't
    /// been created yet — held here so creation can be spread across frames (see `drain_ready`).
    ready_backlog: std::collections::VecDeque<AsyncCompileResult>,
}

impl Default for AsyncHlslCompile {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self {
            inner: std::sync::Mutex::new(AsyncHlslCompileInner {
                tx,
                rx,
                pending: std::collections::HashSet::new(),
                ready_backlog: std::collections::VecDeque::new(),
            }),
        }
    }
}

impl AsyncHlslCompile {
    /// Start a background compile for `shader_id`. No-op if that shader is
    /// already being compiled. Returns true if a new worker was spawned.
    fn spawn(
        &self,
        shader_id: usize,
        hlsl: String,
        cache_key: u64,
        cache_dir: Option<&'static std::path::Path>,
    ) -> bool {
        let tx = {
            let mut inner = self.inner.lock().unwrap();
            if !inner.pending.insert(shader_id) {
                return false;
            }
            inner.tx.clone()
        };
        std::thread::Builder::new()
            .name(format!("hlsl-compile-{}", shader_id))
            .spawn(move || {
                // Discard the bytes once they hit the disk cache — the main
                // thread re-reads them via CxOsDrawShader::new, and keeping
                // them here would pin hundreds of KB per shader until the
                // result is drained.
                let vs_status = get_or_compile_shader_bytes(
                    cache_dir,
                    cache_key,
                    "_vs",
                    "vs_5_0\0",
                    "vertex_main\0",
                    &hlsl,
                )
                .map(drop);
                let ps_status = get_or_compile_shader_bytes(
                    cache_dir,
                    cache_key,
                    "_ps",
                    "ps_5_0\0",
                    "pixel_main\0",
                    &hlsl,
                )
                .map(drop);
                let _ = tx.send(AsyncCompileResult {
                    shader_id,
                    vs_status,
                    ps_status,
                });
            })
            .expect("failed to spawn HLSL compile worker");
        true
    }

    /// Collect any workers that finished since the last call, then hand back at most `budget` of
    /// them for D3D11 object creation this frame (the rest stay queued for following frames so a
    /// burst of finished compiles doesn't stall a single frame). Returns `(results, has_more)`.
    fn drain_ready(&self, budget: usize) -> (Vec<AsyncCompileResult>, bool) {
        debug_assert!(budget >= 1, "drain_ready budget must be >= 1 or the backlog never drains");
        let mut inner = self.inner.lock().unwrap();
        while let Ok(result) = inner.rx.try_recv() {
            inner.pending.remove(&result.shader_id);
            inner.ready_backlog.push_back(result);
        }
        let take = budget.min(inner.ready_backlog.len());
        let out: Vec<AsyncCompileResult> = inner.ready_backlog.drain(..take).collect();
        let has_more = !inner.ready_backlog.is_empty();
        (out, has_more)
    }
}

#[derive(Clone)]
pub struct CxOsDrawShader {
    pub const_table_uniforms: D3d11Buffer,
    pub live_uniforms: D3d11Buffer,
    pub scope_uniforms: D3d11Buffer,
    pub pixel_shader: ID3D11PixelShader,
    pub vertex_shader: ID3D11VertexShader,
    pub pixel_shader_blob: Vec<u8>,
    pub vertex_shader_blob: Vec<u8>,
    pub input_layout: ID3D11InputLayout,
    // Dynamic buffer indices looked up from shader output
    pub draw_call_uniform_buffer_id: Option<u32>,
    pub pass_uniform_buffer_id: Option<u32>,
    pub draw_list_uniform_buffer_id: Option<u32>,
    pub dyn_uniform_buffer_id: Option<u32>,
    pub custom_uniform_buffer_ids: Vec<u32>,
    pub scope_uniform_buffer_id: Option<u32>,
}

impl CxOsDrawShader {
    fn new(
        d3d11_cx: &D3d11Cx,
        hlsl: &str,
        cache_dir: Option<&std::path::Path>,
        mapping: &CxDrawShaderMapping,
        bindings: &UniformBufferBindings,
    ) -> Option<Self> {
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
        // Use the same semantic suffix scheme as the HLSL generator so the
        // InputLayout semantic names match what the compiled vertex shader
        // declares. Single-char `A`..`Z` for the first 26 inputs, then
        // `AA`..`AZ`, `BA`..., which is valid HLSL and keeps names aligned
        // across any number of inputs. Naive `index + 'A'` produces invalid
        // chars (`[`, `\\`, ...) past Z and fails CreateInputLayout with
        // E_INVALIDARG — surfaced by text helpers that have many instance
        // slots.
        use makepad_script::shader_hlsl::index_to_semantic;

        let cache_key = hlsl_cache_key(hlsl);

        let vs_bytes = match get_or_compile_shader_bytes(
            cache_dir,
            cache_key,
            "_vs",
            "vs_5_0\0",
            "vertex_main\0",
            hlsl,
        ) {
            Err(msg) => {
                crate::error!(
                    "Cannot compile vertexshader\n{}\n{}",
                    msg,
                    split_source(hlsl)
                );
                return None;
            }
            Ok(bytes) => bytes,
        };

        let ps_bytes = match get_or_compile_shader_bytes(
            cache_dir,
            cache_key,
            "_ps",
            "ps_5_0\0",
            "pixel_main\0",
            hlsl,
        ) {
            Err(msg) => {
                crate::error!(
                    "Cannot compile pixelshader\n{}\n{}",
                    msg,
                    split_source(hlsl)
                );
                return None;
            }
            Ok(bytes) => bytes,
        };

        let mut vs = None;
        if let Err(e) = unsafe {
            d3d11_cx
                .device
                .CreateVertexShader(&vs_bytes, None, Some(&mut vs))
        } {
            // The DXBC is valid — it just came from the compiler or the on-disk cache — so a
            // failure here is the device, not the shader. Returning `None` puts this shader
            // back in the compile queue for a later frame.
            d3d11_cx.note_error("ID3D11Device::CreateVertexShader", &e);
            return None;
        }

        let mut ps = None;
        if let Err(e) = unsafe {
            d3d11_cx
                .device
                .CreatePixelShader(&ps_bytes, None, Some(&mut ps))
        } {
            d3d11_cx.note_error("ID3D11Device::CreatePixelShader", &e);
            return None;
        }

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
            strings.push(format!("GEOM{}\0", index_to_semantic(geom_sem_index)));
            let semantic_name = PCSTR(strings.last().unwrap().as_ptr());
            let mut slot_offset = 0usize;
            for (semantic_chunk_index, chunk_slots) in
                slot_chunks(geom.slots).into_iter().enumerate()
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
            strings.push(format!("INST{}\0", index_to_semantic(inst_sem_index)));
            let semantic_name = PCSTR(strings.last().unwrap().as_ptr());
            let mut slot_offset = 0usize;
            for (semantic_chunk_index, chunk_slots) in
                slot_chunks(inst.slots).into_iter().enumerate()
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

        if mapping.flags.debug_layout {
            crate::log!(
                "debug_layout d3d11 input_layout: geometry_inputs={} instance_inputs={} total_descs={}",
                mapping.geometries.inputs.len(),
                mapping.instances.inputs.len(),
                layout_debug.len()
            );
            for geom in &mapping.geometries.inputs {
                crate::log!(
                    "debug_layout d3d11 geometry_input id={:?} slots={} offset={} attr={:?}",
                    geom.id,
                    geom.slots,
                    geom.offset,
                    geom.attr_format
                );
            }
            for item in &layout_debug {
                crate::log!("debug_layout d3d11 layout {}", item);
            }
        }

        let mut input_layout = None;
        let input_layout_res = unsafe {
            d3d11_cx
                .device
                .CreateInputLayout(&layout_desc, &vs_bytes, Some(&mut input_layout))
        };
        if let Err(err) = input_layout_res {
            // A mismatched layout is a build-time bug worth shouting about, but a device that
            // died mid-compile fails here too, and killing the process is the one outcome no
            // recovery can undo. Report it and give the shader back to the compile queue.
            crate::error!("Cannot create input layout: {:?}", err);
            crate::error!("Input layout descriptors:");
            for item in &layout_debug {
                crate::error!("  {}", item);
            }
            if std::env::var("MAKEPAD_D3D11_DUMP_HLSL").is_ok() {
                crate::error!("HLSL source\n{}", split_source(hlsl));
            } else {
                crate::error!("Set MAKEPAD_D3D11_DUMP_HLSL=1 to dump full HLSL source.");
            }
            d3d11_cx.note_error("ID3D11Device::CreateInputLayout", &err);
            return None;
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
        let custom_uniform_buffer_ids = mapping
            .uniform_buffers
            .iter()
            .map(|input| input.buffer_index as u32)
            .collect();
        let scope_uniform_buffer_id = bindings.scope_uniform_buffer_index.map(|i| i as u32);

        Some(Self {
            const_table_uniforms,
            live_uniforms,
            scope_uniforms,
            pixel_shader: ps.unwrap(),
            vertex_shader: vs.unwrap(),
            pixel_shader_blob: ps_bytes,
            vertex_shader_blob: vs_bytes,
            input_layout: input_layout.unwrap(),
            draw_call_uniform_buffer_id,
            pass_uniform_buffer_id,
            draw_list_uniform_buffer_id,
            dyn_uniform_buffer_id,
            custom_uniform_buffer_ids,
            scope_uniform_buffer_id,
        })
    }
}
