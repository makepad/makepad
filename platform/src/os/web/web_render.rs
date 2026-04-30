//! Shared web rendering: builds the `FromWasm*` command stream consumed by both
//! [`super::web_gl`] / `web_gl.js` (WebGL2) and [`super::web_gpu`] / `web_gpu.js` (WebGPU).
//!
//! Shader compilation and API-specific setup live in `web_gl.rs` and `web_gpu.rs` respectively.

use crate::{
    cx::Cx,
    draw_list::DrawListId,
    draw_pass::{DrawPassClearColor, DrawPassClearDepth, DrawPassId},
    draw_shader::CxDrawShaderMapping,
    draw_vars::DRAW_CALL_TEXTURE_SLOTS,
    makepad_math::*,
    makepad_wasm_bridge::*,
    os::web::from_wasm::*,
    texture::TextureFormat,
};

impl Cx {
    pub fn render_view(
        &mut self,
        draw_pass_id: DrawPassId,
        draw_list_id: DrawListId,
        zbias: &mut f32,
        zbias_step: f32,
        cmd: &mut Vec<u32>,
    ) {
        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
        let draw_order_len = self.draw_lists[draw_list_id].draw_item_order_len();
        self.draw_lists[draw_list_id]
            .draw_list_uniforms
            .view_transform = Mat4f::identity();

        for order_index in 0..draw_order_len {
            let Some(draw_item_id) =
                self.draw_lists[draw_list_id].draw_item_id_at_order_index(order_index)
            else {
                continue;
            };
            if let Some(sub_list_id) =
                self.draw_lists[draw_list_id].draw_items[draw_item_id].sub_list()
            {
                let child_resets_zbias = self.draw_lists[sub_list_id].reset_zbias;
                let mut child_zbias = 0.0f32;
                self.render_view(
                    draw_pass_id,
                    sub_list_id,
                    if child_resets_zbias {
                        &mut child_zbias
                    } else {
                        zbias
                    },
                    zbias_step,
                    cmd,
                );
            } else {
                let draw_list = &mut self.draw_lists[draw_list_id];
                //view.platform.uni_vw.update_with_f32_data(device, &view.uniforms);
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

                if draw_call.instance_dirty || draw_item.os.inst_vb_id.is_none() {
                    draw_call.instance_dirty = false;
                    if draw_item.os.inst_vb_id.is_none() {
                        draw_item.os.inst_vb_id = Some(self.os.vertex_buffers);
                        self.os.vertex_buffers += 1;
                    }

                    self.os.from_wasm(FromWasmAllocArrayBuffer {
                        buffer_id: draw_item.os.inst_vb_id.unwrap(),
                        data: WasmPtrF32::new(draw_item.instances.as_ref().unwrap()),
                    });
                    draw_call.instance_dirty = false;
                }
                draw_call.draw_call_uniforms.set_zbias(*zbias);
                *zbias += zbias_step;

                // update/alloc textures?
                for i in 0..sh.mapping.textures.len() {
                    let texture_id = if let Some(texture) = &draw_call.texture_slots[i] {
                        texture.texture_id()
                    } else {
                        continue;
                    };

                    let cxtexture = &mut self.textures[texture_id];
                    if cxtexture.format.is_vec() {
                        if cxtexture.alloc_vec() {}
                        if !cxtexture.take_updated().is_empty() {
                            match &cxtexture.format {
                                TextureFormat::VecBGRAu8_32 {
                                    width,
                                    height,
                                    data,
                                    ..
                                } => {
                                    self.os.from_wasm(FromWasmAllocTextureImage2D_BGRAu8_32 {
                                        texture_id: texture_id.0,
                                        width: *width,
                                        height: *height,
                                        data: WasmPtrU32::new((*data).as_ref().unwrap()),
                                    });
                                }
                                TextureFormat::VecMipBGRAu8_32 {
                                    width,
                                    height,
                                    data,
                                    ..
                                } => {
                                    self.os.from_wasm(FromWasmAllocTextureImage2D_BGRAu8_32 {
                                        texture_id: texture_id.0,
                                        width: *width,
                                        height: *height,
                                        data: WasmPtrU32::new((*data).as_ref().unwrap()),
                                    });
                                }
                                TextureFormat::VecRu8 {
                                    width,
                                    height,
                                    data,
                                    ..
                                } => {
                                    self.os.from_wasm(FromWasmAllocTextureImage2D_Ru8 {
                                        texture_id: texture_id.0,
                                        width: *width,
                                        height: *height,
                                        data: WasmPtrU8::new((*data).as_ref().unwrap()),
                                    });
                                }
                                TextureFormat::VecRGBAf32 {
                                    width,
                                    height,
                                    data,
                                    ..
                                } => {
                                    self.os.from_wasm(FromWasmAllocTextureImage2D_RGBAf32 {
                                        texture_id: texture_id.0,
                                        width: *width,
                                        height: *height,
                                        data: WasmPtrF32::new((*data).as_ref().unwrap()),
                                    });
                                }
                                TextureFormat::VecCubeBGRAu8_32 {
                                    width,
                                    height,
                                    data,
                                    ..
                                } => {
                                    self.os.from_wasm(FromWasmAllocTextureCube_BGRAu8_32 {
                                        texture_id: texture_id.0,
                                        width: *width,
                                        height: *height,
                                        data: WasmPtrU32::new((*data).as_ref().unwrap()),
                                    });
                                }
                                _ => continue,
                            }
                        }
                    }
                }

                let geometry_id = if let Some(geometry_id) = draw_call.geometry_id {
                    geometry_id
                } else {
                    continue;
                };

                let geometry = &mut self.geometries[geometry_id];

                if geometry.dirty_vertices || geometry.os.vb_id.is_none() {
                    if geometry.os.vb_id.is_none() {
                        geometry.os.vb_id = Some(self.os.vertex_buffers);
                        self.os.vertex_buffers += 1;
                    }
                    self.os.from_wasm(FromWasmAllocArrayBuffer {
                        buffer_id: geometry.os.vb_id.unwrap(),
                        data: WasmPtrF32::new(&geometry.vertices),
                    });
                    geometry.dirty_vertices = false;
                }

                if geometry.dirty_indices || geometry.os.ib_id.is_none() {
                    if geometry.os.ib_id.is_none() {
                        geometry.os.ib_id = Some(self.os.index_buffers);
                        self.os.index_buffers += 1;
                    }
                    self.os.from_wasm(FromWasmAllocIndexBuffer {
                        buffer_id: geometry.os.ib_id.unwrap(),
                        data: WasmPtrU32::new(&geometry.indices),
                    });
                    geometry.dirty_indices = false;
                }
                geometry.dirty = geometry.dirty_vertices || geometry.dirty_indices;

                // lets check if our vao is still valid
                if draw_item.os.vao.is_none() {
                    draw_item.os.vao = Some(CxOsDrawCallVao {
                        vao_id: self.os.vaos,
                        shader_id: None,
                        inst_vb_id: None,
                        geom_vb_id: None,
                        geom_ib_id: None,
                    });
                    self.os.vaos += 1;
                }

                let vao = draw_item.os.vao.as_mut().unwrap();

                if vao.inst_vb_id != draw_item.os.inst_vb_id
                    || vao.geom_vb_id != geometry.os.vb_id
                    || vao.geom_ib_id != geometry.os.ib_id
                    || vao.shader_id != sh.os_shader_id
                {
                    vao.shader_id = sh.os_shader_id.clone();
                    vao.inst_vb_id = draw_item.os.inst_vb_id;
                    vao.geom_vb_id = geometry.os.vb_id;
                    vao.geom_ib_id = geometry.os.ib_id;

                    self.os.from_wasm(FromWasmAllocVao {
                        vao_id: vao.vao_id,
                        shader_id: vao.shader_id.unwrap(),
                        geom_ib_id: vao.geom_ib_id.unwrap(),
                        geom_vb_id: vao.geom_vb_id.unwrap(),
                        inst_vb_id: draw_item.os.inst_vb_id.unwrap(),
                    });
                }

                let pass_uniforms = &self.passes[draw_pass_id].pass_uniforms;
                let instances = if sh.mapping.instances.total_slots == 0 {
                    0
                } else {
                    draw_item.instances.as_ref().map_or(0, |instances| {
                        instances.len() / sh.mapping.instances.total_slots
                    })
                };
                if sh.mapping.flags.debug_draw && instances > 0 {
                    CxDrawShaderMapping::debug_dump_shader_draw_call(
                        "webgl",
                        draw_item_id,
                        sh,
                        draw_call,
                        draw_item.instances.as_ref().unwrap(),
                        instances,
                    );
                }

                let mut textures = [None; DRAW_CALL_TEXTURE_SLOTS];
                for (index, texture_slot) in draw_call.texture_slots.iter().enumerate() {
                    if let Some(texture) = texture_slot {
                        textures[index] = Some(texture.texture_id().0)
                    }
                }

                // Pack draw calls into a single u32 command buffer to reduce JS dispatch overhead.
                // Format (u32 words):
                // [CMD_DRAW=1,
                //  shader_id, vao_id, depth_write, backface_culling,
                //  pass_ptr, pass_len,
                //  draw_list_ptr, draw_list_len,
                //  draw_call_ptr, draw_call_len,
                //  user_ptr, user_len,
                //  live_ptr, live_len,
                //  tex0..texN (u32 id or 0xFFFF_FFFF)]
                cmd.push(1);
                cmd.push(sh.os_shader_id.unwrap() as u32);
                cmd.push(draw_item.os.vao.as_ref().unwrap().vao_id as u32);
                cmd.push(draw_call.options.depth_write as u32);
                cmd.push(draw_call.options.backface_culling as u32);

                let pass_uniforms = WasmPtrF32::new(pass_uniforms.as_slice());
                cmd.push(pass_uniforms.ptr);
                cmd.push(pass_uniforms.len as u32);
                let draw_list_uniforms = WasmPtrF32::new(draw_list.draw_list_uniforms.as_slice());
                cmd.push(draw_list_uniforms.ptr);
                cmd.push(draw_list_uniforms.len as u32);
                let draw_call_uniforms = WasmPtrF32::new(draw_call.draw_call_uniforms.as_slice());
                cmd.push(draw_call_uniforms.ptr);
                cmd.push(draw_call_uniforms.len as u32);
                let user_uniforms = WasmPtrF32::new(draw_call.dyn_uniforms.as_slice());
                cmd.push(user_uniforms.ptr);
                cmd.push(user_uniforms.len as u32);
                let live_uniforms = WasmPtrF32::new(&sh.mapping.scope_uniforms_buf);
                cmd.push(live_uniforms.ptr);
                cmd.push(live_uniforms.len as u32);

                for tex in textures {
                    cmd.push(tex.unwrap_or(usize::MAX) as u32);
                }
            }
        }
        /*
        if let Some(_) = &self.views[view_id].debug {
            let mut s = String::new();
            self.debug_draw_tree_recur(false, &mut s, view_id, 0);
            console_log(&s);
        }*/
    }

    pub fn setup_render_pass(&mut self, draw_pass_id: DrawPassId, to_texture: bool) -> Vec2d {
        self.passes[draw_pass_id].paint_dirty = false;
        let dpi_factor = self.passes[draw_pass_id].dpi_factor.unwrap();
        let pass_rect = self.get_pass_rect(draw_pass_id, dpi_factor).unwrap();
        let pass = &mut self.passes[draw_pass_id];
        pass.set_dpi_factor(dpi_factor);
        if to_texture {
            // WebGL render-to-texture coordinates are vertically inverted relative to
            // onscreen canvas rendering. Flip Y in the projection for offscreen passes.
            let offset = pass_rect.pos + pass.view_shift;
            let size = pass_rect.size * pass.view_scale;
            pass.pass_uniforms.camera_projection = Mat4f::ortho(
                offset.x as f32,
                (offset.x + size.x) as f32,
                (offset.y + size.y) as f32,
                offset.y as f32,
                100.0,
                -100.0,
                1.0,
                1.0,
            );
            pass.pass_uniforms.camera_view = Mat4f::identity();
        } else {
            if !pass.keep_camera_matrix {
                pass.set_ortho_matrix(pass_rect.pos, pass_rect.size);
            }
        }
        pass_rect.size
    }

    pub fn draw_pass_to_canvas(&mut self, draw_pass_id: DrawPassId) {
        let draw_list_id = self.passes[draw_pass_id].main_draw_list_id.unwrap();

        // get the color and depth
        let clear_color = if self.passes[draw_pass_id].color_textures.len() == 0 {
            self.passes[draw_pass_id].clear_color
        } else {
            match self.passes[draw_pass_id].color_textures[0].clear_color {
                DrawPassClearColor::InitWith(color) => color,
                DrawPassClearColor::ClearWith(color) => color,
            }
        };
        let clear_depth = match self.passes[draw_pass_id].clear_depth {
            DrawPassClearDepth::InitWith(depth) => depth,
            DrawPassClearDepth::ClearWith(depth) => depth,
        };

        self.os.from_wasm(FromWasmBeginRenderCanvas {
            clear_color: clear_color.into(),
            clear_depth,
        });

        self.setup_render_pass(draw_pass_id, false);

        self.os.from_wasm(FromWasmSetDefaultDepthAndBlendMode {});

        let mut zbias = 0.0;
        let zbias_step = self.passes[draw_pass_id].zbias_step;
        let mut cmd_buf = std::mem::take(&mut self.os.render_cmd_buf);
        cmd_buf.clear();
        self.render_view(
            draw_pass_id,
            draw_list_id,
            &mut zbias,
            zbias_step,
            &mut cmd_buf,
        );
        self.os.render_cmd_buf = cmd_buf;
        self.os.from_wasm(FromWasmRenderCommandBuffer {
            words: WasmPtrU32::new(&self.os.render_cmd_buf),
        });
    }

    pub fn draw_pass_to_texture(&mut self, draw_pass_id: DrawPassId) {
        let draw_list_id = self.passes[draw_pass_id].main_draw_list_id.unwrap();

        let pass_size = self.setup_render_pass(draw_pass_id, true);
        let dpi_factor = self.passes[draw_pass_id].dpi_factor.unwrap();
        /*
        self.platform.from_wasm(FromWasmBeginRenderTargets {
            draw_pass_id,
            width: (pass_size.x * dpi_factor) as usize,
            height: (pass_size.y * dpi_factor) as usize
        });*/

        let mut color_targets = [WColorTarget::default()];
        let mut depth_target = WDepthTarget::default();

        for (index, color_texture) in self.passes[draw_pass_id].color_textures.iter().enumerate() {
            let size = pass_size * dpi_factor;
            self.textures[color_texture.texture.texture_id()]
                .alloc_render(size.x as usize, size.y as usize);
            match color_texture.clear_color {
                DrawPassClearColor::InitWith(clear_color) => {
                    color_targets[index] = WColorTarget {
                        texture_id: color_texture.texture.texture_id().0,
                        init_only: true,
                        clear_color: clear_color.into(),
                    };
                }
                DrawPassClearColor::ClearWith(clear_color) => {
                    color_targets[index] = WColorTarget {
                        texture_id: color_texture.texture.texture_id().0,
                        init_only: false,
                        clear_color: clear_color.into(),
                    };
                }
            }
        }

        // attach/clear depth buffers, if any
        if let Some(depth_texture) = &self.passes[draw_pass_id].depth_texture {
            let size = pass_size * dpi_factor;
            self.textures[depth_texture.texture_id()].alloc_depth(size.x as usize, size.y as usize);
            match self.passes[draw_pass_id].clear_depth {
                DrawPassClearDepth::InitWith(clear_depth) => {
                    depth_target = WDepthTarget {
                        texture_id: depth_texture.texture_id().0,
                        init_only: true,
                        clear_depth,
                    };
                }
                DrawPassClearDepth::ClearWith(clear_depth) => {
                    depth_target = WDepthTarget {
                        texture_id: depth_texture.texture_id().0,
                        init_only: false,
                        clear_depth,
                    };
                }
            }
        }

        self.os.from_wasm(FromWasmBeginRenderTexture {
            pass_id: draw_pass_id.0,
            width: (pass_size.x * dpi_factor) as usize,
            height: (pass_size.y * dpi_factor) as usize,
            color_targets,
            depth_target,
        });

        // set the default depth and blendmode
        self.os.from_wasm(FromWasmSetDefaultDepthAndBlendMode {});
        let mut zbias = 0.0;
        let zbias_step = self.passes[draw_pass_id].zbias_step;
        let mut cmd_buf = std::mem::take(&mut self.os.render_cmd_buf);
        cmd_buf.clear();
        self.render_view(
            draw_pass_id,
            draw_list_id,
            &mut zbias,
            zbias_step,
            &mut cmd_buf,
        );
        self.os.render_cmd_buf = cmd_buf;
        self.os.from_wasm(FromWasmRenderCommandBuffer {
            words: WasmPtrU32::new(&self.os.render_cmd_buf),
        });
    }
}

#[derive(Default, Clone, Debug)]
pub struct CxOsPass {}

#[derive(Clone, Default)]
pub struct CxOsDrawList {}

#[derive(Default, Clone)]
pub struct CxOsDrawCallVao {
    pub vao_id: usize,
    pub shader_id: Option<usize>,
    pub inst_vb_id: Option<usize>,
    pub geom_vb_id: Option<usize>,
    pub geom_ib_id: Option<usize>,
}

#[derive(Default, Clone)]
pub struct CxOsDrawCall {
    pub vao: Option<CxOsDrawCallVao>,
    pub inst_vb_id: Option<usize>,
}

#[derive(Clone)]
pub struct CxOsDrawShader {
    pub in_vertex: String,
    pub in_pixel: String,
    pub vertex: String,
    pub pixel: String,
}

#[derive(Clone, Default)]
pub struct CxOsTexture {}

#[derive(Clone, Default)]
pub struct CxOsUniformBuffer {}

#[derive(Clone, Default)]
pub struct CxOsGeometry {
    pub vb_id: Option<usize>,
    pub ib_id: Option<usize>,
}

impl CxOsDrawCall {}

use std::process::Child;
pub fn spawn_process_command(
    _cmd: &str,
    _args: &[&str],
    _current_dir: &str,
) -> Result<Child, std::io::Error> {
    Err(std::io::Error::new(std::io::ErrorKind::NotFound, ""))
}
