use crate::{
    cx::Cx,
    draw_list::DrawListId,
    draw_pass::{DrawPassClearColor, DrawPassClearDepth, DrawPassId},
    draw_shader::{CxDrawShaderCode, CxDrawShaderMapping, DrawShaderInputs},
    draw_vars::DRAW_CALL_TEXTURE_SLOTS,
    makepad_math::*,
    makepad_wasm_bridge::*,
    os::web::from_wasm::*,
    texture::TextureFormat,
};
use std::collections::BTreeSet;

impl Cx {
    pub fn render_view(
        &mut self,
        draw_pass_id: DrawPassId,
        draw_list_id: DrawListId,
        zbias: &mut f32,
        zbias_step: f32,
    ) {
        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
        let draw_order_len = self.draw_lists[draw_list_id].draw_item_order_len();
        // Exploded z-layer view: z is the call's nesting depth, not paint order.
        let sploded = self.passes[draw_pass_id].sploded.is_some();
        // The list's own `view_transform` is the app's (a magnifier well, a
        // render stage matrix) and uploads as set — Metal and GL never reset
        // it; this walk used to overwrite it with the identity, which drew the
        // tweaker's mirrored material at the window's top-left instead of in
        // its well on the web.

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
                let mut own_zbias = 0.0f32;
                let child_zbias = if child_resets_zbias {
                    &mut own_zbias
                } else {
                    &mut *zbias
                };
                // An overlay list carries a depth floor: this is what makes it
                // composite above body content that uses `draw_depth`.
                self.draw_lists[sub_list_id].raise_zbias_to_floor(child_zbias);
                self.render_view(draw_pass_id, sub_list_id, child_zbias, zbias_step);
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

                if sh.mapping.uses_time {
                    self.demo_time_repaint = true;
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
                        byte_data: WasmPtrU8::new(&[]),
                    });
                    draw_call.instance_dirty = false;
                }
                draw_call.resolve_zbias(*zbias, sploded);
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
                                // VecMipBGRAu8_32: level 0 only for now (safe, no mip chain).
                                // Real mips (gl.generateMipmap) are a TODO for the web backend.
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

                if self.geometries.skip_stale(geometry_id) {
                    continue;
                }
                let geometry = &mut self.geometries[geometry_id];
                if !crate::geometry::geometry_layout_matches_shader(
                    geometry,
                    &sh.mapping.geometries,
                ) {
                    continue;
                }

                if geometry.dirty_vertices || geometry.os.vb_id.is_none() {
                    if geometry.os.vb_id.is_none() {
                        geometry.os.vb_id = Some(self.os.vertex_buffers);
                        self.os.vertex_buffers += 1;
                    }
                    match &geometry.vertices {
                        crate::geometry::VertexData::F32(v) => {
                            self.os.from_wasm(FromWasmAllocArrayBuffer {
                                buffer_id: geometry.os.vb_id.unwrap(),
                                data: WasmPtrF32::new(v),
                                byte_data: WasmPtrU8::new(&[]),
                            });
                        }
                        crate::geometry::VertexData::Bytes(v) => {
                            self.os.from_wasm(FromWasmAllocArrayBuffer {
                                buffer_id: geometry.os.vb_id.unwrap(),
                                data: WasmPtrF32::new(&[]),
                                byte_data: WasmPtrU8::new(v),
                            });
                        }
                    }
                    geometry.dirty_vertices = false;
                }

                if geometry.dirty_indices || geometry.os.ib_id.is_none() {
                    if geometry.os.ib_id.is_none() {
                        geometry.os.ib_id = Some(self.os.index_buffers);
                        self.os.index_buffers += 1;
                    }
                    match geometry.index_width {
                        4 => {
                            let Some(v) = geometry.indices.as_u32() else {
                                crate::error!("u32 index staging does not match resident index width");
                                continue;
                            };
                            self.os.from_wasm(FromWasmAllocIndexBuffer {
                                buffer_id: geometry.os.ib_id.unwrap(),
                                data: WasmPtrU32::new(v),
                                byte_data: WasmPtrU8::new(&[]),
                                index_width: 4,
                            });
                        }
                        2 => {
                            let Some(v) = geometry.indices.as_u16() else {
                                crate::error!("u16 index staging does not match resident index width");
                                continue;
                            };
                            self.os.from_wasm(FromWasmAllocIndexBuffer {
                                buffer_id: geometry.os.ib_id.unwrap(),
                                data: WasmPtrU32::new(&[]),
                                byte_data: WasmPtrU8::new(unsafe {
                                    std::slice::from_raw_parts(
                                        v.as_ptr() as *const u8,
                                        v.len() * 2,
                                    )
                                }),
                                index_width: 2,
                            });
                        }
                        width => {
                            crate::error!("invalid resident index width {width}; skipping draw");
                            continue;
                        }
                    }
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

                // A custom-camera texture pass uploads its Y-flipped copy
                // (see `setup_render_pass`); everything else its own.
                let pass_uniforms: &[f32] = match &self.passes[draw_pass_id].os.flipped_uniforms {
                    Some(flipped) => flipped.as_slice(),
                    None => self.passes[draw_pass_id].pass_uniforms.as_slice(),
                };
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

                self.os.from_wasm(FromWasmDrawCall {
                    shader_id: sh.os_shader_id.unwrap(),
                    vao_id: draw_item.os.vao.as_ref().unwrap().vao_id,
                    index_width: geometry.index_width as u32,
                    depth_write: draw_call.options.depth_write,
                    backface_culling: draw_call.options.backface_culling,
                    pass_uniforms: WasmPtrF32::new(pass_uniforms),
                    draw_list_uniforms: WasmPtrF32::new(draw_list.draw_list_uniforms.as_slice()),
                    draw_call_uniforms: WasmPtrF32::new(draw_call.draw_call_uniforms.as_slice()),
                    user_uniforms: WasmPtrF32::new(draw_call.dyn_uniforms.as_slice()),
                    live_uniforms: WasmPtrF32::new(&sh.mapping.scope_uniforms_buf),
                    const_table: WasmPtrF32::new(&[]),
                    textures,
                });
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
        // WebGL render-to-texture coordinates are vertically inverted relative
        // to onscreen canvas rendering: an FBO's rows are stored bottom-up.
        // Every offscreen pass therefore renders with its projection's Y
        // inverted, so the texels land in the same top-left row order Metal
        // and D3D produce and every consumer plain-samples. The JS side pairs
        // this with a clockwise front face for texture passes (the flip
        // reverses triangle winding), so backface culling keeps culling the
        // same faces it culls on the canvas.
        pass.os.flipped_uniforms = None;
        if to_texture {
            if pass.keep_camera_matrix {
                // A custom camera (3D scenes, VJ effects, mesh views): the
                // pass owns its matrices. Overwriting them with the 2D ortho
                // — what this branch did before — drew every 3D scene with a
                // pixel-space projection, which is how the web effect
                // thumbnails came out as their clear colour. Keep the
                // caller's uniforms untouched (the retained draw list
                // re-executes on repaints without the app re-setting the
                // camera, so the flip must never accumulate) and upload a
                // flipped copy instead.
                let mut flipped = pass.pass_uniforms.clone();
                for m in [&mut flipped.camera_projection, &mut flipped.camera_projection_r] {
                    m.v[1] = -m.v[1];
                    m.v[5] = -m.v[5];
                    m.v[9] = -m.v[9];
                    m.v[13] = -m.v[13];
                }
                pass.os.flipped_uniforms = Some(flipped.as_slice().to_vec());
            } else {
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
            }
        } else {
            if !pass.keep_camera_matrix {
                pass.set_ortho_matrix(pass_rect.pos, pass_rect.size);
            }
        }
        pass_rect.size
    }

    pub fn draw_pass_to_canvas(&mut self, draw_pass_id: DrawPassId) {
        // A pass without a draw list (a debug overlay pass that drew nothing this frame) is
        // skipped, as on Metal — unwrapping it took the whole web app down. Its dirt is
        // cleared with it: `setup_render_pass` is what clears it on the normal path, and a
        // pass left dirty here was re-tried — and re-reported — every frame.
        let Some(draw_list_id) = self.passes[draw_pass_id].main_draw_list_id else {
            self.passes[draw_pass_id].paint_dirty = false;
            crate::error!("Draw pass has no draw list!");
            return;
        };

        self.webgl_compile_draw_list_shaders(draw_list_id);

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

        self.render_view(draw_pass_id, draw_list_id, &mut zbias, zbias_step);
    }

    pub fn draw_pass_to_texture(&mut self, draw_pass_id: DrawPassId) {
        // A pass without a draw list (a debug overlay pass that drew nothing this frame) is
        // skipped, as on Metal — unwrapping it took the whole web app down. Settled, as in
        // `draw_pass_to_canvas`.
        let Some(draw_list_id) = self.passes[draw_pass_id].main_draw_list_id else {
            self.passes[draw_pass_id].paint_dirty = false;
            crate::error!("Draw pass has no draw list!");
            return;
        };

        self.webgl_compile_draw_list_shaders(draw_list_id);

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
            // Attachment format for the JS side: R32F float targets need a
            // different texImage2D (and EXT_color_buffer_float).
            let format = match &self.textures[color_texture.texture.texture_id()].format {
                TextureFormat::RenderRf32 { .. } => 1,
                _ => 0,
            };
            match color_texture.clear_color {
                DrawPassClearColor::InitWith(clear_color) => {
                    color_targets[index] = WColorTarget {
                        texture_id: color_texture.texture.texture_id().0,
                        init_only: true,
                        clear_color: clear_color.into(),
                        format,
                    };
                }
                DrawPassClearColor::ClearWith(clear_color) => {
                    color_targets[index] = WColorTarget {
                        texture_id: color_texture.texture.texture_id().0,
                        init_only: false,
                        clear_color: clear_color.into(),
                        format,
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
                        attached: true,
                        texture_id: depth_texture.texture_id().0,
                        init_only: true,
                        clear_depth,
                    };
                }
                DrawPassClearDepth::ClearWith(clear_depth) => {
                    depth_target = WDepthTarget {
                        attached: true,
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

        self.render_view(draw_pass_id, draw_list_id, &mut zbias, zbias_step);
    }

    fn webgl_collect_draw_list_shaders(
        &self,
        draw_list_id: DrawListId,
        draw_shader_ids: &mut BTreeSet<usize>,
    ) {
        let draw_list = &self.draw_lists[draw_list_id];
        for order_index in 0..draw_list.draw_item_order_len() {
            let Some(draw_item_id) = draw_list.draw_item_id_at_order_index(order_index) else {
                continue;
            };
            let draw_item = &draw_list.draw_items[draw_item_id];
            if let Some(sub_list_id) = draw_item.sub_list() {
                self.webgl_collect_draw_list_shaders(sub_list_id, draw_shader_ids);
            } else if let Some(draw_call) = draw_item.kind.draw_call() {
                draw_shader_ids.insert(draw_call.draw_shader_id.index);
            }
        }
    }

    /// Queue only programs referenced by the draw-list tree for this pass.
    /// Shader objects can be registered long before their widgets are visible;
    /// compiling the global registry here made the first WebGL frame pay for
    /// every hidden screen and template.
    fn webgl_compile_draw_list_shaders(&mut self, draw_list_id: DrawListId) {
        let mut draw_shader_ids = BTreeSet::new();
        self.webgl_collect_draw_list_shaders(draw_list_id, &mut draw_shader_ids);

        for draw_shader_id in draw_shader_ids {
            if self.draw_shaders.shaders[draw_shader_id]
                .os_shader_id
                .is_some()
            {
                self.draw_shaders.compile_set.remove(&draw_shader_id);
                continue;
            }

            let (vertex, pixel, geometry_slots, instance_slots, textures, debug_code, geom_attribs, inst_attribs) = {
                let cx_shader = &self.draw_shaders.shaders[draw_shader_id];
                let (vertex, pixel) = match &cx_shader.mapping.code {
                    CxDrawShaderCode::Separate { vertex, fragment } => {
                        (vertex.clone(), fragment.clone())
                    }
                    CxDrawShaderCode::Combined { .. } => {
                        crate::error!("Combined shader code is not supported on wasm webgl");
                        self.draw_shaders.compile_set.remove(&draw_shader_id);
                        continue;
                    }
                };
                let textures: Vec<WTextureInput> = cx_shader
                    .mapping
                    .textures
                    .iter()
                    .map(|v| v.to_from_wasm_texture_input())
                    .collect();
                let compact = cx_shader.mapping.geometry_is_compact()
                    || cx_shader.mapping.instances.has_compact();
                let (geom_attribs, inst_attribs) = if compact {
                    (
                        Self::webgl_typed_attribs("geom", &cx_shader.mapping.geometries),
                        Self::webgl_typed_attribs("inst", &cx_shader.mapping.instances),
                    )
                } else {
                    (Vec::new(), Vec::new())
                };
                (
                    vertex,
                    pixel,
                    cx_shader.mapping.geometries.total_slots,
                    cx_shader.mapping.instances.total_slots,
                    textures,
                    cx_shader.mapping.flags.debug_code,
                    geom_attribs,
                    inst_attribs,
                )
            };

            if debug_code {
                crate::log!("{}\n{}", vertex, pixel);
            }

            let mut os_shader_id = self.draw_shaders.shaders[draw_shader_id].os_shader_id;
            if os_shader_id.is_none() {
                for (index, ds) in self.draw_shaders.os_shaders.iter().enumerate() {
                    if ds.in_vertex == vertex && ds.in_pixel == pixel {
                        os_shader_id = Some(index);
                        break;
                    }
                }
            }

            if os_shader_id.is_none() {
                let shp = CxOsDrawShader::new(vertex, pixel);
                let shader_id = self.draw_shaders.os_shaders.len();
                self.os.from_wasm(FromWasmCompileWebGLShader {
                    shader_id,
                    vertex: shp.vertex.clone(),
                    pixel: shp.pixel.clone(),
                    geometry_slots,
                    instance_slots,
                    textures,
                    geom_attribs,
                    inst_attribs,
                });
                self.draw_shaders.os_shaders.push(shp);
                self.os.webgl_shaders_pending += 1;
                os_shader_id = Some(shader_id);
            }

            self.draw_shaders.shaders[draw_shader_id].os_shader_id = os_shader_id;
            self.draw_shaders.compile_set.remove(&draw_shader_id);
        }
    }

    fn webgl_typed_attribs(prefix: &str, inputs: &DrawShaderInputs) -> Vec<WVertexAttrib> {
        let stride = if inputs.stride_bytes != 0 {
            inputs.stride_bytes
        } else {
            inputs.total_slots * 4
        };
        inputs
            .inputs
            .iter()
            .map(|input| WVertexAttrib {
                name: format!("{}_{}", prefix, input.id),
                offset: input.byte_offset as u32,
                size: input.attr_format.component_count() as u32,
                stride: stride as u32,
                gl_type: input.attr_format.gl_type_code(),
                normalized: if input.attr_format.is_normalized() {
                    1
                } else {
                    0
                },
                integer: if input.attr_format.is_integer_fetch() {
                    1
                } else {
                    0
                },
            })
            .collect()
    }
}

impl CxOsDrawShader {
    pub fn new(in_vertex: String, in_pixel: String) -> Self {
        let vertex = format!(
            "#version 300 es
#define VIEW_ID 0
precision highp float;
precision highp int;
vec4 sample2d(sampler2D sampler, vec2 pos){{return texture(sampler, vec2(pos.x, pos.y));}}
vec4 sample2d_lod(sampler2D sampler, vec2 pos, float lod){{return textureLod(sampler, vec2(pos.x, pos.y), lod);}}
vec4 sample2d_bgra(sampler2D sampler, vec2 pos){{return texture(sampler, vec2(pos.x, pos.y));}}
vec4 samplecube(samplerCube sampler, vec3 dir){{return texture(sampler, dir);}}
vec4 samplecube_lod(samplerCube sampler, vec3 dir, float lod){{return textureLod(sampler, dir, lod);}}
vec4 samplecube_bgra(samplerCube sampler, vec3 dir){{return texture(sampler, dir);}}
vec4 depth_clip(vec4 w, vec4 c, float clip){{return c;}}
{}",
            in_vertex
        );

        let pixel = format!(
            "#version 300 es
#define VIEW_ID 0
precision highp float;
precision highp int;
vec4 sample2d(sampler2D sampler, vec2 pos){{return texture(sampler, vec2(pos.x, pos.y));}}
vec4 sample2d_lod(sampler2D sampler, vec2 pos, float lod){{return textureLod(sampler, vec2(pos.x, pos.y), lod);}}
vec4 sample2d_bgra(sampler2D sampler, vec2 pos){{return texture(sampler, vec2(pos.x, pos.y));}}
vec4 samplecube(samplerCube sampler, vec3 dir){{return texture(sampler, dir);}}
vec4 samplecube_lod(samplerCube sampler, vec3 dir, float lod){{return textureLod(sampler, dir, lod);}}
vec4 samplecube_bgra(samplerCube sampler, vec3 dir){{return texture(sampler, dir);}}
vec4 depth_clip(vec4 w, vec4 c, float clip){{return c;}}
{}",
            in_pixel
        );

        Self {
            in_vertex,
            in_pixel,
            vertex,
            pixel,
        }
    }
}

#[derive(Default, Clone, Debug)]
pub struct CxOsPass {
    /// The pass uniforms a custom-camera (`keep_camera_matrix`) texture pass
    /// actually uploads: the caller's matrices with the projection's Y
    /// inverted for WebGL's bottom-up render targets. `None` for canvas
    /// passes and for 2D texture passes, whose ortho is built flipped. Kept
    /// as the upload slice (`DrawPassUniforms::as_slice`).
    pub flipped_uniforms: Option<Vec<f32>>,
}

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
