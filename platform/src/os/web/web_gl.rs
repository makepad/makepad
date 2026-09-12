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

const WEBGL_RESOURCE_RETIREMENT_SLOTS_PER_SAFE_POINT: usize = 32;

impl Cx {
    pub(crate) fn has_pending_webgl_resource_retirements(&self) -> bool {
        self.geometries.0.has_pending_retirements()
            || self.draw_lists.has_pending_instance_retirements()
            || self.passes.0.has_pending_retirements()
            || self.textures.0.has_pending_retirements()
    }

    /// Drain a bounded number of pool slots at an event-loop safe point. A
    /// candidate is produced only by the last owning handle being dropped;
    /// allocation cancels it, so no draw-age heuristic is involved.
    pub(crate) fn retire_webgl_resources(&mut self) {
        self.draw_lists.1.allocations.collect_for_frame(
            self.repaint_id,
            self.textures
                .1
                .serials
                .completed
                .load(std::sync::atomic::Ordering::Acquire),
        );

        let mut array_buffer_ids = Vec::new();
        let mut index_buffer_ids = Vec::new();
        let mut vao_ids = Vec::new();
        let mut texture_ids = Vec::new();
        let mut framebuffer_ids = Vec::new();

        for _ in 0..WEBGL_RESOURCE_RETIREMENT_SLOTS_PER_SAFE_POINT {
            let Some(slot) = self.geometries.0.take_free_retirement() else {
                continue;
            };
            let geometry = &mut self.geometries.0.pool[slot].item;
            if let Some(id) = geometry.os.vb_id {
                array_buffer_ids.push(id);
                geometry.os.vb_generation = geometry.os.vb_generation.wrapping_add(1);
            }
            if let Some(id) = geometry.os.ib_id {
                index_buffer_ids.push(id);
                geometry.os.ib_generation = geometry.os.ib_generation.wrapping_add(1);
            }
            // Keep the numeric ids for same-slot reuse, but force a complete
            // upload before any new generation can draw them.
            geometry.dirty = true;
            geometry.dirty_vertices = true;
            geometry.dirty_indices = true;
        }

        self.draw_lists
            .retire_free_items(&self.task_pool(), self.repaint_id, |os| {
                if let Some(id) = os.inst_vb_id {
                    array_buffer_ids.push(id);
                    os.inst_vb_generation = os.inst_vb_generation.wrapping_add(1);
                }
                if let Some(vao) = &mut os.vao {
                    vao_ids.push(vao.vao_id);
                    // Preserve the id high-water mark while ensuring reuse
                    // emits FromWasmAllocVao and recreates its two UBOs.
                    vao.shader_id = None;
                    vao.inst_vb_id = None;
                    vao.geom_vb_id = None;
                    vao.geom_ib_id = None;
                }
                os.uniforms_recording_gen = None;
                os.draw_call_uniforms_gen = None;
                os.user_uniforms_gen = None;
                os.inst_capacity = 0;
                os.inst_charge.take()
            });

        for _ in 0..WEBGL_RESOURCE_RETIREMENT_SLOTS_PER_SAFE_POINT {
            let Some(slot) = self.passes.0.take_free_retirement() else {
                continue;
            };
            framebuffer_ids.push(slot);
            let pass = &mut self.passes.0.pool[slot].item;
            // Release only small owning handles and stale graph edges. Large
            // texture/geometry CPU staging remains in its own pool slot.
            pass.color_textures.clear();
            pass.depth_texture = None;
            pass.main_draw_list_id = None;
            pass.parent = crate::draw_pass::CxDrawPassParent::None;
            pass.attached_by = None;
            pass.paint_dirty = false;
            pass.live_with_parent = false;
            pass.repaint_requested = false;
            pass.os.flipped_uniforms = None;
        }

        // Draw-list and pass cleanup above can release their final Texture Rc.
        for _ in 0..WEBGL_RESOURCE_RETIREMENT_SLOTS_PER_SAFE_POINT {
            let Some(slot) = self.textures.0.take_free_retirement() else {
                continue;
            };
            texture_ids.push(slot);
        }

        if array_buffer_ids.is_empty()
            && index_buffer_ids.is_empty()
            && vao_ids.is_empty()
            && texture_ids.is_empty()
            && framebuffer_ids.is_empty()
        {
            return;
        }
        self.os.from_wasm(FromWasmFreeWebGLResources {
            array_buffer_ids,
            index_buffer_ids,
            vao_ids,
            texture_ids,
            framebuffer_ids,
        });
    }

    pub fn render_view(
        &mut self,
        draw_pass_id: DrawPassId,
        draw_list_id: DrawListId,
        zbias: &mut f32,
        zbias_step: f32,
    ) {
        if !self.draw_lists.1.allocations.has_device_limit() {
            // WebGL deliberately exposes no VRAM query. Use the browser's
            // conservative process allowance and identify this fallback.
            let allowance = self.memory_budget();
            self.draw_lists
                .1
                .allocations
                .set_device_limit(allowance / 4);
            // The one derived limit of the publication registry (contract §7).
            self.publications.set_envelope(allowance / 4);
            crate::log!("retained-upload budgets: process_allowance={} allocation_limit={} source=web_process_allowance_fallback", allowance, allowance / 4);
        }
        let shaders_pending = self.os.webgl_shaders_pending != 0;
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
            let uniforms_gen = self.next_uniform_gen();
            let Some(draw_item_id) =
                self.draw_lists[draw_list_id].draw_item_id_at_order_index(order_index)
            else {
                continue;
            };
            if let Some(sub_list_id) =
                self.draw_lists[draw_list_id].draw_items[draw_item_id].sub_list()
            {
                // A retained sub-list its owner dropped between the parent's
                // last record and this paint: the slot may already hold
                // another widget's list. Nothing to draw here.
                if self.draw_lists.is_id_freed(sub_list_id) {
                    continue;
                }
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
                // A retained list is one unit of paint order: its calls all
                // take the counter at entry, it advances by the layers the
                // list reported. See `CxDrawList::zbias_hold`.
                if let Some(steps) = self.draw_lists[sub_list_id].zbias_hold {
                    let mut held = *child_zbias;
                    self.render_view(draw_pass_id, sub_list_id, &mut held, 0.0);
                    *child_zbias += steps as f32 * zbias_step;
                } else {
                    self.render_view(draw_pass_id, sub_list_id, child_zbias, zbias_step);
                }
            } else {
                let (draw_list, upload_budget) =
                    self.draw_lists.list_and_upload_budget(draw_list_id);
                let draw_list_recording_gen = draw_list.recording_gen;
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

                if (draw_call.instance_dirty
                    || draw_item.os.inst_vb_id.is_none()
                    || draw_item.retained_gpu_evicted)
                    && !(draw_item.retained_gpu_evicted
                        && draw_item.retained_instances.is_some()
                        && draw_item.retained_instance_count == 0)
                {
                    upload_budget.allocations.collect_for_frame(
                        self.repaint_id,
                        self.textures
                            .1
                            .serials
                            .completed
                            .load(std::sync::atomic::Ordering::Acquire),
                    );
                    let bytes = draw_item.retained_instances.as_ref().map_or_else(
                        || draw_item.instances.as_ref().map_or(0, |v| v.len() * 4),
                        |p| p.byte_len(),
                    );
                    let replaces = draw_item.retained_instances.is_none()
                        || draw_item.retained_upload_range.start == 0
                        || draw_item.os.inst_vb_id.is_none()
                        || bytes > draw_item.os.inst_capacity;
                    if replaces {
                        let capacity = if draw_item.retained_instances.is_some() {
                            bytes.next_power_of_two().max(256)
                        } else {
                            bytes
                        };
                        let Some(charge) = upload_budget.allocations.reserve(capacity) else {
                            draw_item.instance_upload_pending = true;
                            self.demo_time_repaint = true;
                            continue;
                        };
                        draw_item.os.inst_capacity = capacity;
                        draw_item.os.inst_charge = Some(charge);
                    }
                    draw_item.instance_upload_pending = false;
                    draw_call.instance_dirty = false;
                    draw_item.retained_instance_id =
                        draw_item.retained_instances.as_ref().map_or(0, |v| v.id());
                    draw_item.resident_schema = draw_item.retained_schema;
                    draw_item.retained_gpu_evicted = false;
                    if draw_item.os.inst_vb_id.is_none() {
                        draw_item.os.inst_vb_id = Some(self.os.vertex_buffers);
                        self.os.vertex_buffers += 1;
                    }

                    if let Some(retained) = &draw_item.retained_instances {
                        self.os.from_wasm(FromWasmRetainedArrayBuffer {
                            buffer_id: draw_item.os.inst_vb_id.unwrap(),
                            data: WasmPtrF32::new(retained.data()),
                            first_slot: draw_item.retained_upload_range.start,
                        });
                    } else {
                        self.os.from_wasm(FromWasmAllocArrayBuffer {
                            buffer_id: draw_item.os.inst_vb_id.unwrap(),
                            data: WasmPtrF32::new(draw_item.instances.as_deref().unwrap()),
                            byte_data: WasmPtrU8::new(&[]),
                        });
                    }
                    draw_call.instance_dirty = false;
                    draw_item.retained_instance_id =
                        draw_item.retained_instances.as_ref().map_or(0, |v| v.id());
                    draw_item.resident_schema = draw_item.retained_schema;
                    draw_item.retained_gpu_evicted = false;
                }
                draw_call.resolve_zbias(*zbias, sploded, uniforms_gen);
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
                                        data: WasmPtrU32::new(match data {
                                            Some(data) => data,
                                            None => continue,
                                        }),
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
                                        data: WasmPtrU32::new(match data {
                                            Some(data) => data,
                                            None => continue,
                                        }),
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
                                        data: WasmPtrU8::new(match data {
                                            Some(data) => data,
                                            None => continue,
                                        }),
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
                                        data: WasmPtrF32::new(match data {
                                            Some(data) => data,
                                            None => continue,
                                        }),
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
                                        data: WasmPtrU32::new(match data {
                                            Some(data) => data,
                                            None => continue,
                                        }),
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

                // A freed-but-not-yet-reused slot still has the same pool
                // generation. Web retirement may already have deleted its
                // buffers, so retained draw ids must treat FREE as stale too.
                if self.geometries.0.is_free(geometry_id.slot_index())
                    || self.geometries.skip_stale(geometry_id)
                {
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
                                crate::error!(
                                    "u32 index staging does not match resident index width"
                                );
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
                                crate::error!(
                                    "u16 index staging does not match resident index width"
                                );
                                continue;
                            };
                            self.os.from_wasm(FromWasmAllocIndexBuffer {
                                buffer_id: geometry.os.ib_id.unwrap(),
                                data: WasmPtrU32::new(&[]),
                                byte_data: WasmPtrU8::new(unsafe {
                                    std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 2)
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
                        inst_vb_generation: 0,
                        geom_vb_generation: 0,
                        geom_ib_generation: 0,
                    });
                    self.os.vaos += 1;
                }

                let vao = draw_item.os.vao.as_mut().unwrap();

                if vao.inst_vb_id != draw_item.os.inst_vb_id
                    || vao.geom_vb_id != geometry.os.vb_id
                    || vao.geom_ib_id != geometry.os.ib_id
                    || vao.shader_id != sh.os_shader_id
                    || vao.inst_vb_generation != draw_item.os.inst_vb_generation
                    || vao.geom_vb_generation != geometry.os.vb_generation
                    || vao.geom_ib_generation != geometry.os.ib_generation
                {
                    vao.shader_id = sh.os_shader_id.clone();
                    vao.inst_vb_id = draw_item.os.inst_vb_id;
                    vao.geom_vb_id = geometry.os.vb_id;
                    vao.geom_ib_id = geometry.os.ib_id;
                    vao.inst_vb_generation = draw_item.os.inst_vb_generation;
                    vao.geom_vb_generation = geometry.os.vb_generation;
                    vao.geom_ib_generation = geometry.os.ib_generation;

                    self.os.from_wasm(FromWasmAllocVao {
                        vao_id: vao.vao_id,
                        shader_id: vao.shader_id.unwrap(),
                        geom_ib_id: vao.geom_ib_id.unwrap(),
                        geom_vb_id: vao.geom_vb_id.unwrap(),
                        inst_vb_id: draw_item.os.inst_vb_id.unwrap(),
                    });
                    draw_item.os.uniforms_recording_gen = None;
                    draw_item.os.draw_call_uniforms_gen = None;
                    draw_item.os.user_uniforms_gen = None;
                }

                // A custom-camera texture pass uploads its Y-flipped copy
                // (see `setup_render_pass`); everything else its own.
                let pass_uniforms: &[f32] = match &self.passes[draw_pass_id].os.flipped_uniforms {
                    Some(flipped) => flipped.as_slice(),
                    None => self.passes[draw_pass_id].pass_uniforms.as_slice(),
                };
                let instances = if sh.mapping.instances.total_slots == 0 {
                    0
                } else if draw_item.retained_instances.is_some() {
                    draw_item.retained_instance_count
                } else {
                    draw_item.instances.as_ref().map_or(0, |instances| {
                        instances.len() / sh.mapping.instances.total_slots
                    })
                };
                if instances == 0 {
                    continue;
                }
                if sh.mapping.flags.debug_draw {
                    CxDrawShaderMapping::debug_dump_shader_draw_call(
                        "webgl",
                        draw_item_id,
                        sh,
                        draw_call,
                        draw_item
                            .retained_instances
                            .as_ref()
                            .map(|v| v.data())
                            .unwrap_or_else(|| draw_item.instances.as_deref().unwrap()),
                        instances,
                    );
                }

                let mut textures = [None; DRAW_CALL_TEXTURE_SLOTS];
                for (index, texture_slot) in draw_call.texture_slots.iter().enumerate() {
                    if let Some(texture) = texture_slot {
                        textures[index] = Some(texture.texture_id().0)
                    }
                }

                let reset_draw_uniforms =
                    draw_item.os.uniforms_recording_gen != Some(draw_list_recording_gen);
                let upload_draw_call_uniforms = reset_draw_uniforms
                    || draw_item.os.draw_call_uniforms_gen != Some(draw_call.uniforms_gen);
                let upload_user_uniforms = reset_draw_uniforms
                    || draw_item.os.user_uniforms_gen != Some(draw_call.uniforms_gen);
                let draw_call_uniforms: &[f32] = if upload_draw_call_uniforms {
                    draw_call.draw_call_uniforms.as_slice()
                } else {
                    &[]
                };
                let user_uniforms = if upload_user_uniforms {
                    draw_call.dyn_uniforms.as_slice()
                } else {
                    &[]
                };
                if !shaders_pending {
                    draw_item.os.uniforms_recording_gen = Some(draw_list_recording_gen);
                    if upload_draw_call_uniforms {
                        draw_item.os.draw_call_uniforms_gen = Some(draw_call.uniforms_gen);
                    }
                    if upload_user_uniforms {
                        draw_item.os.user_uniforms_gen = Some(draw_call.uniforms_gen);
                    }
                }

                let pass_uniforms_gen = self.passes[draw_pass_id].pass_uniforms_gen;
                let draw_list_uniforms_gen = draw_list.uniforms_gen;
                let uniforms_gen = draw_call.uniforms_gen;
                let live_uniforms_gen = sh.mapping.scope_uniforms_gen;
                debug_assert_ne!(pass_uniforms_gen, 0);
                debug_assert_ne!(draw_list_uniforms_gen, 0);
                debug_assert_ne!(uniforms_gen, 0);
                debug_assert_ne!(live_uniforms_gen, 0);

                self.os.from_wasm(FromWasmDrawCall {
                    custom_uniforms: sh
                        .mapping
                        .uniform_buffers
                        .iter()
                        .enumerate()
                        .map(|(slot, input)| {
                            let uniform = draw_call.uniform_buffer_slots[slot]
                                .as_ref()
                                .map(|b| &self.uniform_buffers[b.uniform_buffer_id()]);
                            WCustomUniformBuffer {
                                block_name: input.block_name.clone(),
                                data: WasmPtrU8::new(uniform.map_or(&[], |b| b.data.as_slice())),
                                generation_lo: uniform.map_or(0, |b| b.generation as u32),
                                generation_hi: uniform.map_or(0, |b| (b.generation >> 32) as u32),
                            }
                        })
                        .collect(),
                    shader_id: sh.os_shader_id.unwrap(),
                    vao_id: draw_item.os.vao.as_ref().unwrap().vao_id,
                    index_width: geometry.index_width as u32,
                    depth_write: draw_call.options.depth_write,
                    alpha_blend: draw_call.options.alpha_blend,
                    backface_culling: draw_call.options.backface_culling,
                    pass_uniforms: WasmPtrF32::new(pass_uniforms),
                    pass_uniforms_gen_lo: pass_uniforms_gen as u32,
                    pass_uniforms_gen_hi: (pass_uniforms_gen >> 32) as u32,
                    draw_list_uniforms: WasmPtrF32::new(draw_list.draw_list_uniforms.as_slice()),
                    draw_list_uniforms_gen_lo: draw_list_uniforms_gen as u32,
                    draw_list_uniforms_gen_hi: (draw_list_uniforms_gen >> 32) as u32,
                    draw_call_uniforms: WasmPtrF32::new(draw_call_uniforms),
                    draw_call_uniforms_gen_lo: uniforms_gen as u32,
                    draw_call_uniforms_gen_hi: (uniforms_gen >> 32) as u32,
                    user_uniforms: WasmPtrF32::new(user_uniforms),
                    user_uniforms_gen_lo: uniforms_gen as u32,
                    user_uniforms_gen_hi: (uniforms_gen >> 32) as u32,
                    live_uniforms: WasmPtrF32::new(&sh.mapping.scope_uniforms_buf),
                    live_uniforms_gen_lo: live_uniforms_gen as u32,
                    live_uniforms_gen_hi: (live_uniforms_gen >> 32) as u32,
                    reset_draw_uniforms,
                    const_table: WasmPtrF32::new(&[]),
                    textures,
                });
                draw_item.consumed_instance_id = draw_item.retained_instance_id;
                draw_item.consumed_schema = draw_item.resident_schema;
                draw_item.consumed_serial = if self.os.webgl_shaders_pending == 0 {
                    self.textures
                        .1
                        .serials
                        .submitted
                        .load(std::sync::atomic::Ordering::Acquire)
                        + 1
                } else {
                    0
                };
                if let Some(charge) = &draw_item.os.inst_charge {
                    charge.submitted(draw_item.consumed_serial);
                }
                if let Some((block, _)) = draw_item.shared.as_ref() {
                    // The lease's receipt under the pass's serial; the frame
                    // frontier (`poll_texture_lifetimes`) completes it.
                    let receipt = block.receipt();
                    receipt.mark_encoded(draw_item.consumed_serial);
                    receipt.mark_submitted(
                        draw_item.consumed_serial,
                        draw_item.kind.draw_call().map_or(0, |call| call.uniforms_gen),
                    );
                }
                draw_item.consumed_uniforms_gen = draw_item
                    .kind
                    .draw_call()
                    .map_or(0, |call| call.uniforms_gen);
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
        // the bake transaction's paint receipt (whole draws: ranges ignored)
        self.passes[draw_pass_id].painted_serial = self.repaint_id;
        let dpi_factor = self.passes[draw_pass_id].dpi_factor.unwrap();
        let pass_rect = self.get_pass_rect(draw_pass_id, dpi_factor).unwrap();
        let dpi_uniforms_gen = self.next_uniform_gen();
        let changed_uniforms_gen = self.next_uniform_gen();
        let ortho_uniforms_gen = self.next_uniform_gen();
        let pass = &mut self.passes[draw_pass_id];
        pass.set_dpi_factor(dpi_factor, dpi_uniforms_gen);
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
                flip_projection_y(&mut flipped.camera_projection);
                flip_projection_y(&mut flipped.camera_projection_r);
                pass.os.flipped_uniforms = Some(flipped.as_slice().to_vec());
                pass.mark_pass_uniforms_dirty(changed_uniforms_gen);
            } else {
                // The 2D camera is built in ONE place, `set_ortho_matrix`,
                // on every backend — it is also where the exploded view's
                // `camera_view` comes from (`crate::sploded`). Building the
                // ortho by hand here, with the identity for `camera_view`,
                // dropped that camera for the exploded BODY pass, which
                // renders through a texture: its draw calls sit at
                // `nesting_depth * SPLODED_DEPTH_UNIT` in z, which without
                // the explode camera's z scale lies far outside the ortho's
                // clip range, so every one of them was clipped away and the
                // web showed a bare window where Metal drew the stack. Same
                // matrix as the canvas, then the Y inversion for the
                // bottom-up target, as the custom-camera branch above.
                pass.set_ortho_matrix(pass_rect.pos, pass_rect.size, changed_uniforms_gen);
                flip_projection_y(&mut pass.pass_uniforms.camera_projection);
            }
        } else {
            if !pass.keep_camera_matrix {
                pass.set_ortho_matrix(pass_rect.pos, pass_rect.size, ortho_uniforms_gen);
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
        if self.draw_lists.is_id_freed(draw_list_id) {
            self.passes[draw_pass_id].paint_dirty = false;
            return;
        }

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
        let serial = self.textures.1.serials.submit();
        self.readback_pass_submitted(draw_pass_id, serial);
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
        if self.draw_lists.is_id_freed(draw_list_id) {
            self.passes[draw_pass_id].paint_dirty = false;
            return;
        }

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
                TextureFormat::RenderRGBAf32 { .. } => 2,
                TextureFormat::RenderRGBAf16 { .. } => 3,
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
        let serial = self.textures.1.serials.submit();
        self.readback_pass_submitted(draw_pass_id, serial);
        if !self.textures.1.readbacks.slots.is_empty() {
            self.web_capture_texture_readbacks(Some(draw_pass_id));
        }
    }

    fn webgl_collect_draw_list_shaders(
        &self,
        draw_list_id: DrawListId,
        draw_shader_ids: &mut BTreeSet<usize>,
    ) {
        // A retained sub-list its owner dropped since the parent last
        // recorded: not part of this pass (see `render_view`).
        if self.draw_lists.is_id_freed(draw_list_id) {
            return;
        }
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

            let (
                vertex,
                pixel,
                geometry_slots,
                instance_slots,
                textures,
                debug_code,
                geom_attribs,
                inst_attribs,
            ) = {
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
precision highp sampler2DShadow;
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
precision highp sampler2DShadow;
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

/// WebGL renders a texture pass into a bottom-up target: negate the
/// projection's Y row so the texels land in the top-left row order Metal and
/// D3D produce (see `Cx::setup_render_pass`).
fn flip_projection_y(m: &mut Mat4f) {
    m.v[1] = -m.v[1];
    m.v[5] = -m.v[5];
    m.v[9] = -m.v[9];
    m.v[13] = -m.v[13];
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
    pub inst_vb_generation: u64,
    pub geom_vb_generation: u64,
    pub geom_ib_generation: u64,
}

#[derive(Default, Clone)]
pub struct CxOsDrawCall {
    pub vao: Option<CxOsDrawCallVao>,
    pub inst_vb_id: Option<usize>,
    pub inst_vb_generation: u64,
    pub inst_capacity: usize,
    pub inst_charge: Option<crate::retained_instances::RetainedAllocation>,
    pub uniforms_recording_gen: Option<u64>,
    pub draw_call_uniforms_gen: Option<u64>,
    pub user_uniforms_gen: Option<u64>,
}

impl CxOsDrawCall {
    /// This backend keeps no per-publication backing lease on a draw item
    /// (contract §10): nothing to release when the item's lease clears.
    pub(crate) fn take_backing(&mut self) -> Option<u64> {
        None
    }
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
    pub vb_generation: u64,
    pub ib_generation: u64,
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

impl CxOsTexture {
    pub(crate) fn allocated_bytes(&self, _cx: &Cx) -> Option<u64> {
        None
    }
}
impl Cx {
    pub(crate) fn texture_allocation_bytes(&self, _id: crate::texture::TextureId) -> Option<u64> {
        // JS can reject uploads and scale render targets to hardware/safety
        // limits. Rust TextureAlloc is a request, not an allocation receipt.
        // Reporting it as actual bytes would let consumers under-budget.
        None
    }

    pub(crate) fn release_texture_allocation(&mut self, id: crate::texture::TextureId) {
        if !(self.textures[id].format.is_render()
            || self.textures[id].format.is_vec()
            || self.textures[id].format.is_depth())
        {
            return;
        }
        let framebuffer_ids = self
            .passes
            .0
            .pool
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                let pass = &slot.item;
                (pass
                    .color_textures
                    .iter()
                    .any(|color| color.texture.texture_id() == id)
                    || pass
                        .depth_texture
                        .as_ref()
                        .is_some_and(|depth| depth.texture_id() == id))
                .then_some(index)
            })
            .collect();
        // WebGL deletion releases the table name now; the driver retains the
        // actual object until previously queued commands are done. Delete all
        // FBO references too. Future use creates a new WebGL object at this id.
        self.os.from_wasm(FromWasmFreeWebGLResources {
            texture_ids: vec![id.0],
            framebuffer_ids,
            array_buffer_ids: Vec::new(),
            index_buffer_ids: Vec::new(),
            vao_ids: Vec::new(),
        });
        self.textures[id].reset_allocation();
        self.textures[id].os = Default::default();
        self.textures[id].previous_platform_resource = None;
    }

    pub(crate) fn poll_texture_lifetimes(&mut self) {
        // The adapter draws attached blocks here (no per-publication
        // backing): dropped blocks release from this poll, contract §3.3.
        self.publications.retire_without_backing();
        let completed = self
            .textures
            .1
            .serials
            .completed
            .load(std::sync::atomic::Ordering::Acquire);
        let submitted = self.frame_submission_serial();
        if submitted > completed && self.os.completion_pending == 0 {
            self.os.completion_pending = submitted;
            self.os.from_wasm(FromWasmPollGpuCompletion {
                serial_lo: submitted as u32,
                serial_hi: (submitted >> 32) as u32,
            });
        }
        self.textures
            .1
            .retired
            .retain(|retired| retired.serial > completed);
    }
}
