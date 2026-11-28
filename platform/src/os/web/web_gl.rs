use crate::{
    cx::Cx,
    draw_list::DrawListId,
    draw_pass::{DrawPassClearColor, DrawPassClearDepth, DrawPassId},
    draw_shader::{CxDrawShaderCode, CxDrawShaderMapping, DrawShaderBlendOp, DrawShaderColorFormat, DrawShaderInputs},
    draw_vars::DRAW_CALL_TEXTURE_SLOTS,
    makepad_math::*,
    makepad_wasm_bridge::*,
    os::web::from_wasm::*,
    os::web::{to_wasm::ToWasmInstanceUploadFailed, CxOs, SlabBacking},
    draw_list::{CxDrawItems, ItemRegion},
    instance_store::{RegionRef, RegionUse, SlabEvent},
    retained_instances::RetainedAllocationBudget,
    texture::{CxTexturePool, TextureFormat, TextureId, TextureUpdated},
};
use std::collections::BTreeSet;

const WEBGL_RESOURCE_RETIREMENT_SLOTS_PER_SAFE_POINT: usize = 32;

impl Cx {
    pub(crate) fn has_pending_webgl_resource_retirements(&self) -> bool {
        self.geometries.0.has_pending_retirements()
            || self.draw_lists.has_pending_instance_retirements()
            || self.passes.0.has_pending_retirements()
            || self.textures.0.has_pending_retirements()
            || !self.textures.1.post_upload_discard.is_empty()
    }

    /// Drain a bounded number of pool slots at an event-loop safe point. A
    /// candidate is produced only by the last owning handle being dropped;
    /// allocation cancels it, so no draw-age heuristic is involved.
    pub(crate) fn retire_webgl_resources(&mut self) {
        let discard = std::mem::take(&mut self.textures.1.post_upload_discard);
        for id in discard {
            if self.textures.0.is_live_generation(id.0, id.generation()) {
                let tex = &mut self.textures[id];
                if tex.discard_cpu_after_upload && tex.updated().is_empty() {
                    tex.format.discard_cpu_data();
                }
            }
        }
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
            geometry.vertices = Default::default();
            geometry.indices = Default::default();
            geometry.dirty = true;
            geometry.dirty_vertices = true;
            geometry.dirty_indices = true;
        }

        // Retired items' plain regions, then the freed items themselves: a publication's last
        // user releases its region. Their VAOs go; the rows' slabs stay with the store.
        self.draw_lists.release_retired_regions(&mut self.os.regions.store);
        let pool = self.task_pool();
        let regions = &mut self.os.regions;
        self.draw_lists
            .retire_free_items_with_ids(&pool, self.repaint_id, |list, index, item| {
                regions.release_item((list.index(), index), item);
                let os = &mut item.os;
                if let Some(vao) = &mut os.vao {
                    vao_ids.push(vao.vao_id);
                    // Preserve the id high-water mark while ensuring reuse
                    // emits FromWasmAllocVao.
                    vao.shader_id = None;
                    vao.inst = None;
                    vao.geom_vb_id = None;
                    vao.geom_ib_id = None;
                }
                os.retained_failed = 0;
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
            pass.readers.clear();
            pass.paint_dirty = false;
            pass.live_with_parent = false;
            pass.repaint_requested = false;
            pass.repaint_next = false;
            pass.time_owed = false;
        }

        // Draw-list and pass cleanup above can release their final Texture Rc.
        for _ in 0..WEBGL_RESOURCE_RETIREMENT_SLOTS_PER_SAFE_POINT {
            let Some(slot) = self.textures.0.take_free_retirement() else {
                continue;
            };
            let tex = &mut self.textures.0.pool[slot].item;
            tex.format = TextureFormat::Unknown;
            tex.alloc = None;
            tex.animation = None;
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
            instance_slabs: Vec::new(),
            array_buffer_ids,
            index_buffer_ids,
            vao_ids,
            texture_ids,
            framebuffer_ids,
        });
    }

    /// After the browser restored a lost WebGL context: every object of the
    /// old one is gone and the page's tables of them are empty. What the CPU
    /// side keeps is marked to upload again on its next draw (vec textures
    /// from their data, geometries, instance buffers and VAOs). Programs are
    /// compiled again on first draw, render targets are allocated by their next pass, and every pass
    /// with a draw list repaints. The old context's frames are over, so every
    /// submitted serial counts as complete (the upload budget frees what they
    /// held). Buffer, VAO and texture ids keep their numbers: an upload to an
    /// id the page no longer has creates its object anew.
    pub(crate) fn webgl_forget_gpu_resources(&mut self) {
        for slot in &mut self.textures.0.pool {
            let texture = &mut slot.item;
            if texture.format.is_vec() || texture.format.is_render() || texture.format.is_depth() {
                texture.reset_allocation();
            }
            texture.os = Default::default();
            texture.previous_platform_resource = None;
        }
        for slot in &mut self.geometries.0.pool {
            let geometry = &mut slot.item;
            geometry.dirty = true;
            geometry.dirty_vertices = true;
            geometry.dirty_indices = true;
            geometry.os.vb_generation = geometry.os.vb_generation.wrapping_add(1);
            geometry.os.ib_generation = geometry.os.ib_generation.wrapping_add(1);
        }
        // The page's slabs went with the context: every region is invalid, and so is every
        // publication binding. Items write their rows again on their next draw.
        self.os.regions.store.drop_all();
        self.os.regions.store.take_events();
        self.os.regions.clear_publications();
        self.os.slab_backings.clear();
        for slot in &mut self.draw_lists.0.pool {
            let draw_list = &mut slot.item;
            for index in 0..draw_list.draw_items.len() {
                let draw_item = &mut draw_list.draw_items[index];
                if let Some(draw_call) = draw_item.kind.draw_call_mut() {
                    draw_call.instance_dirty = true;
                }
                let os = &mut draw_item.os;
                if let Some(vao) = &mut os.vao {
                    vao.shader_id = None;
                    vao.inst = None;
                    vao.geom_vb_id = None;
                    vao.geom_ib_id = None;
                }
                os.retained_failed = 0;
                draw_item.retained_gpu_evicted = true;
            }
        }
        for slot in &mut self.passes.0.pool {
            let pass = &mut slot.item;
            if pass.main_draw_list_id.is_some() {
                pass.paint_dirty = true;
            }
        }
        self.draw_shaders.os_shaders.clear();
        for shader in &mut self.draw_shaders.shaders {
            shader.pipeline_id = None;
        }
        self.os.webgl_shaders_pending = 0;
        self.os.webgl_shaders_waiting = 0;
        self.os.completion_pending = 0;
        let submitted = self.frame_submission_serial();
        self.textures.1.serials.complete(submitted);
        self.textures.1.retired.clear();
    }

    pub fn render_view(
        &mut self,
        draw_pass_id: DrawPassId,
        draw_list_id: DrawListId,
        pass_uniforms: WUniformRange,
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
        let mut draw_list_uniforms: Option<WUniformRange> = None;
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
                // a slot that holds a sub-list now lets go of the rows it drew before
                let key = (draw_list_id.index(), draw_item_id);
                self.os.regions.release_item(key, self.draw_lists[draw_list_id].draw_items.binding_mut(draw_item_id));
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
                    self.render_view(draw_pass_id, sub_list_id, pass_uniforms, &mut held, 0.0);
                    *child_zbias += steps as f32 * zbias_step;
                } else {
                    self.render_view(draw_pass_id, sub_list_id, pass_uniforms, child_zbias, zbias_step);
                }
            } else {
                // the reader serial of this pass: it submits after its walk
                let serial = self
                    .textures
                    .1
                    .serials
                    .submitted
                    .load(std::sync::atomic::Ordering::Acquire)
                    + 1;
                let (draw_list, upload_budget) =
                    self.draw_lists.list_and_upload_budget(draw_list_id);
                let key = (draw_list_id.index(), draw_item_id);
                let Some(shader_index) = draw_list.draw_items[draw_item_id]
                    .kind
                    .draw_call()
                    .map(|call| call.draw_shader_id.index)
                else {
                    // an emptied slot draws nothing: what it drew goes
                    self.os
                        .regions
                        .release_item(key, draw_list.draw_items.binding_mut(draw_item_id));
                    continue;
                };
                let sh = &self.draw_shaders[shader_index];
                if sh.pipeline_id.is_none() {
                    // shader didnt compile somehow
                    continue;
                }
                let slots = sh.mapping.instances.total_slots;
                let count = {
                    let item = &draw_list.draw_items[draw_item_id];
                    // A publication whose region write the page refused stays off screen
                    // until its content changes: a new publication gets a fresh attempt.
                    if item.os.retained_failed != 0
                        && item
                            .retained_instances
                            .as_ref()
                            .is_some_and(|publication| publication.id() == item.os.retained_failed)
                    {
                        continue;
                    }
                    if item.retained_instances.is_some() {
                        item.retained_instance_count
                    } else if slots == 0 {
                        0
                    } else {
                        item.instances.as_ref().map_or(0, |rows| rows.len() / slots)
                    }
                };
                if count == 0 || slots == 0 {
                    // nothing to draw, nothing owed: the content is consumed as it is, and
                    // the rows the item drew before go
                    let item = draw_list.draw_items.binding_mut(draw_item_id);
                    self.os.regions.release_item(key, item);
                    item.instance_upload_pending = false;
                    item.retained_gpu_evicted = false;
                    item.retained_instance_id =
                        item.retained_instances.as_ref().map_or(0, |p| p.id());
                    item.resident_schema = item.retained_schema;
                    if let Some(call) = item.kind.draw_call_mut() {
                        call.instance_dirty = false;
                        call.resolve_zbias(*zbias, sploded, uniforms_gen);
                    }
                    *zbias += zbias_step;
                    continue;
                }
                let copy_started = Cx::monotonic_now();
                let (held, uploaded) = self.os.instance_rows(
                    &mut upload_budget.allocations,
                    &mut draw_list.draw_items,
                    WebInstanceDraw { key, list_gen: draw_list_id.generation(), slots, count, serial },
                );
                if uploaded != 0 {
                    upload_budget.stats.bytes =
                        upload_budget.stats.bytes.saturating_add(uploaded as usize);
                    upload_budget.stats.install_us = upload_budget.stats.install_us.saturating_add(
                        ((Cx::monotonic_now() - copy_started).max(0.0) * 1_000_000.0) as u64,
                    );
                }
                let draw_item = &mut draw_list.draw_items[draw_item_id];
                let draw_call = draw_item.kind.draw_call_mut().unwrap();
                draw_call.resolve_zbias(*zbias, sploded, uniforms_gen);
                *zbias += zbias_step;
                let Some(held) = held else { continue };

                // update/alloc textures?
                for i in 0..sh.mapping.textures.len() {
                    let texture_id = if let Some(texture) = &draw_call.texture_slots[i] {
                        texture.texture_id()
                    } else {
                        continue;
                    };
                    if crate::os::cx_shared::samples_incomplete_target(
                        &self.passes,
                        &self.textures,
                        texture_id,
                        draw_pass_id,
                        self.repaint_id,
                    ) {
                        self.passes[draw_pass_id].paint_dirty = true;
                    }

                    Self::webgl_upload_vec_texture(&mut self.textures, &mut self.os, texture_id);
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

                let mut instance_ranges = Vec::<WInstanceRange>::new();
                draw_item.draw_ranges(held.rows, &mut instance_ranges);
                if instance_ranges.is_empty() {
                    continue;
                }

                // lets check if our vao is still valid
                if draw_item.os.vao.is_none() {
                    draw_item.os.vao = Some(CxOsDrawCallVao {
                        vao_id: self.os.vaos,
                        ..Default::default()
                    });
                    self.os.vaos += 1;
                }

                let vao = draw_item.os.vao.as_mut().unwrap();
                let inst = WebInstanceBinding {
                    slab: held.slab,
                    generation: self.os.slab_backings[held.slab as usize].generation,
                    region_index: held.region.index,
                    region_gen: held.region.gen,
                    byte_off: u64::from(held.first_row) * u64::from(held.stride),
                    byte_len: u64::from(held.rows) * u64::from(held.stride),
                };
                // A move inside the same backing keeps the VAO and updates its instance offset.
                let rebinds = vao.geom_vb_id != geometry.os.vb_id
                    || vao.geom_ib_id != geometry.os.ib_id
                    || vao.shader_id != sh.pipeline_id
                    || vao.geom_vb_generation != geometry.os.vb_generation
                    || vao.geom_ib_generation != geometry.os.ib_generation
                    || vao.inst.is_none_or(|old| (old.slab, old.generation) != (inst.slab, inst.generation));
                if rebinds || vao.inst != Some(inst) {
                    vao.shader_id = sh.pipeline_id.clone();
                    vao.inst = Some(inst);
                    vao.geom_vb_id = geometry.os.vb_id;
                    vao.geom_ib_id = geometry.os.ib_id;
                    vao.geom_vb_generation = geometry.os.vb_generation;
                    vao.geom_ib_generation = geometry.os.ib_generation;

                    self.os.from_wasm(FromWasmAllocVao {
                        vao_id: vao.vao_id,
                        shader_id: vao.shader_id.unwrap(),
                        geom_ib_id: vao.geom_ib_id.unwrap(),
                        geom_vb_id: vao.geom_vb_id.unwrap(),
                        slab_slot: inst.slab,
                        backing_generation: inst.generation,
                        region: WInstanceRegion { index: inst.region_index, gen: inst.region_gen },
                        byte_off: inst.byte_off as usize,
                        byte_len: inst.byte_len as usize,
                        instance_byte_offset: inst.byte_off as usize,
                    });
                }

                let draw_call = draw_item.kind.draw_call().unwrap();
                if sh.mapping.flags.debug_draw {
                    let data = draw_item.retained_instances.as_ref()
                        .map(|publication| publication.data())
                        .unwrap_or_else(|| draw_item.instances.as_deref().unwrap());
                    for range in &instance_ranges {
                        CxDrawShaderMapping::debug_dump_shader_draw_call(
                            "webgl", draw_item_id, sh, draw_call,
                            &data[range.first as usize * slots..], range.count as usize,
                        );
                    }
                }

                let mut textures = [None; DRAW_CALL_TEXTURE_SLOTS];
                for (index, texture_slot) in draw_call.texture_slots.iter().enumerate() {
                    if let Some(texture) = texture_slot {
                        textures[index] = Some(texture.texture_id().0)
                    }
                }

                let draw_list_uniforms = *draw_list_uniforms.get_or_insert_with(|| {
                    self.os.uniforms
                        .put_f32s(draw_list.draw_list_uniforms.as_slice())
                        .into()
                });
                let draw_call_uniforms = self.os.uniforms
                    .put_draw_call(draw_call.draw_call_uniforms.as_slice())
                    .into();
                let user_uniforms = self.os.uniforms
                    .put_dyn(draw_call.uniforms_gen, &draw_call.dyn_uniforms)
                    .into();
                let live_uniforms = self.os.uniforms
                    .put_scope(shader_index, sh.mapping.scope_uniforms_gen, &sh.mapping.scope_uniforms_buf)
                    .into();
                self.os.from_wasm(FromWasmDrawCall {
                    instance_ranges,
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
                    shader_id: sh.pipeline_id.unwrap(),
                    vao_id: draw_item.os.vao.as_ref().unwrap().vao_id,
                    index_width: geometry.index_width as u32,
                    depth_write: draw_call.options.depth_write,
                    depth_test_greater: draw_call.options.depth_test_greater,
                    alpha_blend: draw_call.options.alpha_blend && (sh.mapping.color_format.blends() || sh.mapping.blend_op == DrawShaderBlendOp::Add),
                    blend_max: sh.mapping.blend_op == DrawShaderBlendOp::Max,
                    blend_add: sh.mapping.blend_op == DrawShaderBlendOp::Add,
                    cull_mode: draw_call.options.cull_mode as u32,
                    pass_uniforms,
                    draw_list_uniforms,
                    draw_call_uniforms,
                    user_uniforms,
                    live_uniforms,
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
                if sh.mapping.uses_time {
                    let time_fps = draw_item.kind.draw_call().map_or(0.0, |call| call.options.time_fps);
                    self.demo_time_repaint |=
                        self.passes[draw_pass_id].request_time_repaint(time_fps, &mut self.time_fps_clock);
                }
            }
        }
        // a list that recorded fewer items lets go of the discarded tail's rows
        self.os.regions.release_spares(draw_list_id.index(), &mut self.draw_lists[draw_list_id].draw_items);
        /*
        if let Some(_) = &self.views[view_id].debug {
            let mut s = String::new();
            self.debug_draw_tree_recur(false, &mut s, view_id, 0);
            console_log(&s);
        }*/
    }

    /// A vec texture's pending data, sent to WebGL whole (a draw samples it, or it is staged).
    fn webgl_upload_vec_texture(textures: &mut CxTexturePool, os: &mut CxOs, texture_id: TextureId) {
        let cxtexture = &mut textures[texture_id];
        if cxtexture.format.is_vec() {
            // Leave the update pending while the data's taken, so its rect
            // merges into the next upload instead of getting lost.
            if let TextureFormat::VecBGRAu8_32 { data: None, .. } = cxtexture.format {
                return;
            }
            let updated = cxtexture.take_updated();
            if !updated.is_empty() {
                // An empty update mustn't use up a size change,
                // or the rect below would miss the texture.
                let is_new_allocation = cxtexture.alloc_vec();
                match &cxtexture.format {
                    TextureFormat::VecBGRAu8_32 {
                        width,
                        height,
                        data: Some(data),
                        ..
                    } => {
                        if let (TextureUpdated::Partial(rect), false) = (updated, is_new_allocation) {
                            os.from_wasm(FromWasmUpdateTextureRect_BGRAu8_32 {
                                texture_id: texture_id.0,
                                image_width: *width,
                                image_height: *height,
                                x: rect.origin.x,
                                y: rect.origin.y,
                                width: rect.size.width,
                                height: rect.size.height,
                                data: WasmPtrU32::new(data),
                            });
                            return;
                        }
                        os.from_wasm(FromWasmAllocTextureImage2D_BGRAu8_32 {
                            texture_id: texture_id.0,
                            width: *width,
                            height: *height,
                            data: WasmPtrU32::new(data),
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
                        os.from_wasm(FromWasmAllocTextureImage2D_BGRAu8_32 {
                            texture_id: texture_id.0,
                            width: *width,
                            height: *height,
                            data: WasmPtrU32::new(match data {
                                Some(data) => data.as_slice(),
                                None => return,
                            }),
                        });
                    }
                    TextureFormat::VecRu8 {
                        width,
                        height,
                        data,
                        ..
                    } => {
                        os.from_wasm(FromWasmAllocTextureImage2D_Ru8 {
                            texture_id: texture_id.0,
                            width: *width,
                            height: *height,
                            data: WasmPtrU8::new(match data {
                                Some(data) => data,
                                None => return,
                            }),
                        });
                    }
                    TextureFormat::VecRGBAf32 {
                        width,
                        height,
                        data,
                        ..
                    } => {
                        os.from_wasm(FromWasmAllocTextureImage2D_RGBAf32 {
                            texture_id: texture_id.0,
                            width: *width,
                            height: *height,
                            data: WasmPtrF32::new(match data {
                                Some(data) => data,
                                None => return,
                            }),
                        });
                    }
                    // Level 0 only, as VecMipBGRAu8_32 above: the layers'
                    // level 0s must be contiguous (no mip chain in the data).
                    TextureFormat::VecArrayMipBGRAu8_32 {
                        width,
                        height,
                        layers,
                        data,
                        max_level,
                        ..
                    } => {
                        if max_level.unwrap_or(0) > 0 {
                            crate::error!("web: a 2D-array texture with mips is not uploaded (give max_level 0)");
                            return;
                        }
                        os.from_wasm(FromWasmAllocTextureArray_BGRAu8_32 {
                            texture_id: texture_id.0,
                            width: *width,
                            height: *height,
                            layers: *layers,
                            data: WasmPtrU32::new(match data {
                                Some(data) => data.as_slice(),
                                None => return,
                            }),
                        });
                    }
                    TextureFormat::VecCubeBGRAu8_32 {
                        width,
                        height,
                        data,
                        ..
                    } => {
                        os.from_wasm(FromWasmAllocTextureCube_BGRAu8_32 {
                            texture_id: texture_id.0,
                            width: *width,
                            height: *height,
                            data: WasmPtrU32::new(match data {
                                Some(data) => data,
                                None => return,
                            }),
                        });
                    }
                    _ => return,
                }
                if textures[texture_id].discard_cpu_after_upload {
                    textures.1.post_upload_discard.push(texture_id);
                }
            }
        }
    }

    /// The staged textures ([`crate::Texture::set_staged_upload`]) that wait to upload, sent
    /// whether a draw samples them or not.
    pub(crate) fn webgl_staged_uploads(&mut self) {
        let ids = self.take_staged_textures();
        for &id in &ids {
            if !self.textures[id].updated().is_empty() {
                Self::webgl_upload_vec_texture(&mut self.textures, &mut self.os, id);
            }
        }
        self.staged_textures = ids;
    }

    pub fn setup_render_pass(&mut self, draw_pass_id: DrawPassId, to_texture: bool) -> Vec2d {
        self.passes[draw_pass_id].paint_dirty = false;
        // the bake transaction's paint receipt
        self.passes[draw_pass_id].painted_serial = self.repaint_id;
        // its targets hold this repaint's picture for the passes after it
        self.passes[draw_pass_id].painted_repaint = self.repaint_id;
        let dpi_factor = self.passes[draw_pass_id].dpi_factor.unwrap();
        let pass_rect = self.get_pass_rect(draw_pass_id, dpi_factor).unwrap();
        let dpi_uniforms_gen = self.next_uniform_gen();
        let changed_uniforms_gen = self.next_uniform_gen();
        let ortho_uniforms_gen = self.next_uniform_gen();
        let pass = &mut self.passes[draw_pass_id];
        pass.set_dpi_factor(dpi_factor, dpi_uniforms_gen);
        // The same camera as every other backend: a custom camera (3D
        // scenes, VJ effects, mesh views) is the pass's own, a 2D pass gets
        // the ortho built in ONE place, `set_ortho_matrix` (also where the
        // exploded view's `camera_view` comes from, `crate::sploded`). A
        // framebuffer's rows are stored bottom-up; the inversion that answers
        // that is applied to every vertex shader's clip position in texture
        // passes (the GLSL `mp_clip_y_flip`, set by the JS draw for
        // `FromWasmBeginRenderTexture` passes, with a clockwise front face),
        // not to the projection: shaders that write clip space themselves
        // (fullscreen graph passes, line renderers, light-map bakes) never
        // read the projection, and inverting it alone stored their pictures
        // upside down.
        if !pass.keep_camera_matrix {
            pass.set_ortho_matrix(
                pass_rect.pos,
                pass_rect.size,
                if to_texture { changed_uniforms_gen } else { ortho_uniforms_gen },
            );
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

        let pass_uniforms = self.os.uniforms
            .put_f32s(self.passes[draw_pass_id].pass_uniforms.as_slice())
            .into();
        self.render_view(draw_pass_id, draw_list_id, pass_uniforms, &mut zbias, zbias_step);
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

        let mut color_targets: Vec<WColorTarget> = Vec::new();
        let mut depth_target = WDepthTarget::default();

        for color_texture in self.passes[draw_pass_id].color_textures.iter() {
            let size = pass_size * dpi_factor;
            self.textures[color_texture.texture.texture_id()]
                .alloc_render(size.x as usize, size.y as usize);
            // Attachment format for the JS side: R32F float targets need a
            // different texImage2D (and EXT_color_buffer_float).
            let format = match &self.textures[color_texture.texture.texture_id()].format {
                TextureFormat::RenderRf32 { .. } => 1,
                TextureFormat::RenderRGBAf32 { .. } => 2,
                TextureFormat::RenderRGBAf16 { .. } => 3,
                TextureFormat::RenderRf16 { .. } => 4,
                TextureFormat::RenderRu8 { .. } => 5,
                _ => 0,
            };
            match color_texture.clear_color {
                DrawPassClearColor::InitWith(clear_color) => {
                    color_targets.push(WColorTarget {
                        texture_id: color_texture.texture.texture_id().0,
                        init_only: true,
                        clear_color: clear_color.into(),
                        format,
                    });
                }
                DrawPassClearColor::ClearWith(clear_color) => {
                    color_targets.push(WColorTarget {
                        texture_id: color_texture.texture.texture_id().0,
                        init_only: false,
                        clear_color: clear_color.into(),
                        format,
                    });
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

        let pass_uniforms = self.os.uniforms
            .put_f32s(self.passes[draw_pass_id].pass_uniforms.as_slice())
            .into();
        self.render_view(draw_pass_id, draw_list_id, pass_uniforms, &mut zbias, zbias_step);
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
                .pipeline_id
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
                let mapping = &cx_shader.mapping;
                let textures = match mapping.glsl_texture_bindings.iter()
                    .map(|binding| WTextureInput::from_binding(mapping, binding))
                    .collect::<Result<Vec<_>, _>>()
                {
                    Ok(textures) => textures,
                    Err(error) => {
                        crate::error!("WebGL shader {}: {}", cx_shader.debug_id, error);
                        self.draw_shaders.compile_set.remove(&draw_shader_id);
                        continue;
                    }
                };
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

            let shader = &self.draw_shaders.shaders[draw_shader_id];
            let color_format = shader.mapping.color_format;
            let blend_op = shader.mapping.blend_op;
            let outputs = shader.mapping.fragment_outputs as u32;
            let shader_name = shader.debug_id.to_string();
            let mut pipeline_id = shader.pipeline_id;
            if pipeline_id.is_none() {
                for (index, ds) in self.draw_shaders.os_shaders.iter().enumerate() {
                    if ds.in_vertex == vertex && ds.in_pixel == pixel && ds.color_format == color_format && ds.blend_op == blend_op && ds.textures == textures {
                        pipeline_id = Some(index);
                        break;
                    }
                }
            }

            if pipeline_id.is_none() {
                let shp = CxOsDrawShader::new(vertex, pixel, color_format, blend_op, textures);
                let shader_id = self.draw_shaders.os_shaders.len();
                self.os.from_wasm(FromWasmCompileWebGLShader {
                    shader_id,
                    shader_name,
                    color_format: match color_format {
                        DrawShaderColorFormat::Rf32 => 1,
                        DrawShaderColorFormat::Rgba32F => 2,
                        DrawShaderColorFormat::Rgba16F => 3,
                        DrawShaderColorFormat::Rf16 => 4,
                        DrawShaderColorFormat::Ru8 => 5,
                        DrawShaderColorFormat::Bgra8Unorm | DrawShaderColorFormat::Bgra8NoBlend => 0,
                    },
                    blend_add: blend_op == DrawShaderBlendOp::Add,
                    outputs,
                    vertex: shp.vertex.clone(),
                    pixel: shp.pixel.clone(),
                    geometry_slots,
                    instance_slots,
                    textures: shp.textures.clone(),
                    geom_attribs,
                    inst_attribs,
                });
                self.draw_shaders.os_shaders.push(shp);
                self.os.webgl_shaders_pending += 1;
                pipeline_id = Some(shader_id);
            }

            self.draw_shaders.shaders[draw_shader_id].pipeline_id = pipeline_id;
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

/// One draw's instance rows: whose they are and what this draw reads.
struct WebInstanceDraw {
    key: (usize, usize),
    list_gen: u64,
    slots: usize,
    count: usize,
    serial: u64,
}

fn f32_bytes(rows: &[f32]) -> &[u8] {
    // SAFETY: f32 has no padding, and u8 has alignment 1.
    unsafe { std::slice::from_raw_parts(rows.as_ptr().cast(), std::mem::size_of_val(rows)) }
}

impl CxOs {
    /// The store's slab events, in stream order: a Destroy of a slot goes out before its
    /// next Create. Each Create charges its whole capacity once; Destroy drops that charge,
    /// which the budget collects once its last reader completed.
    fn apply_slab_events(&mut self, budget: &mut RetainedAllocationBudget) {
        let mut destroyed = Vec::new();
        for event in self.regions.store.take_events() {
            match event {
                SlabEvent::Create { slab, bytes } => {
                    self.free_slabs(&mut destroyed);
                    self.last_backing_generation = self.last_backing_generation.wrapping_add(1).max(1);
                    let generation = self.last_backing_generation;
                    let at = slab as usize;
                    if self.slab_backings.len() <= at {
                        self.slab_backings.resize_with(at + 1, Default::default);
                    }
                    self.slab_backings[at] = SlabBacking { generation, charge: Some(budget.reserve_physical(bytes)) };
                    self.from_wasm(FromWasmCreateInstanceSlab {
                        slab_slot: slab,
                        backing_generation: generation,
                        capacity_bytes: bytes as f64,
                    });
                }
                SlabEvent::Destroy { slab } => {
                    if let Some(backing) = self.slab_backings.get_mut(slab as usize) {
                        if backing.generation != 0 {
                            destroyed.push(WInstanceSlab { slab_slot: slab, backing_generation: backing.generation });
                        }
                        *backing = SlabBacking::default();
                    }
                }
            }
        }
        self.free_slabs(&mut destroyed);
    }

    fn free_slabs(&mut self, slabs: &mut Vec<WInstanceSlab>) {
        if slabs.is_empty() {
            return;
        }
        self.from_wasm(FromWasmFreeWebGLResources {
            instance_slabs: std::mem::take(slabs),
            array_buffer_ids: Vec::new(),
            index_buffer_ids: Vec::new(),
            vao_ids: Vec::new(),
            texture_ids: Vec::new(),
            framebuffer_ids: Vec::new(),
        });
    }

    fn write_region(&mut self, held: ItemRegion, byte_off: u64, bytes: u64, parts: Vec<WasmPtrU8>) {
        let generation = self.slab_backings.get(held.slab as usize).map_or(0, |b| b.generation);
        self.from_wasm(FromWasmWriteInstanceRegion {
            slab_slot: held.slab,
            backing_generation: generation,
            region: WInstanceRegion { index: held.region.index, gen: held.region.gen },
            byte_off: byte_off as usize,
            byte_len: bytes as usize,
            parts,
        });
    }

    /// The region and its backing's charge stay until this draw's serial completes.
    fn pin(&mut self, held: ItemRegion, serial: u64) {
        self.regions.store.drawn(held.region, serial);
        if let Some(charge) = self.slab_backings.get(held.slab as usize).and_then(|b| b.charge.as_ref()) {
            charge.submitted(serial);
        }
    }

    /// Make an item's rows resident in the page's slabs and pin them through this draw's
    /// serial, as native's `fb_instances` does: a publication prefix (or a shared block, whole)
    /// is written once for all its users, a recording into a fresh region when it changed.
    /// Returns the item's region and the bytes written.
    fn instance_rows(
        &mut self,
        budget: &mut RetainedAllocationBudget,
        items: &mut CxDrawItems,
        draw: WebInstanceDraw,
    ) -> (Option<ItemRegion>, u64) {
        let item = items.binding_mut(draw.key.1);
        if item.retained_instances.is_some() {
            self.regions.release_plain(item);
        }
        if let Some(publication) = &item.retained_instances {
            let id = publication.id();
            let block = item.shared.as_ref().map(|(block, _)| block);
            let (held, written) =
                match self.regions.publication(draw.key, draw.list_gen, publication, block, draw.count, draw.slots) {
                    RegionUse::Reuse(held) => (held, 0),
                    RegionUse::Write { held, byte_off, bytes } => {
                        self.apply_slab_events(budget);
                        let parts = match block {
                            Some(block) => vec![WasmPtrU8::new(f32_bytes(block.data()))],
                            None => publication
                                .data_slices(0..(bytes / 4) as usize)
                                .map(|(_, part)| WasmPtrU8::new(f32_bytes(part)))
                                .collect(),
                        };
                        self.write_region(held, byte_off, bytes, parts);
                        if let Some(block) = block {
                            block.receipt().mark_ready();
                        }
                        (held, bytes)
                    }
                };
            self.pin(held, draw.serial);
            item.instance_upload_pending = false;
            item.retained_gpu_evicted = false;
            item.retained_instance_id = id;
            item.resident_schema = item.retained_schema;
            if let Some(call) = item.kind.draw_call_mut() {
                call.instance_dirty = false;
            }
            return (Some(held), written);
        }
        // an item that stopped being retained lets go of its publication region
        self.regions.release_publication(draw.key);
        let decision = self.regions.recording(items, draw.key.1, draw.count, draw.slots);
        let item = &mut items.buffer[draw.key.1];
        if decision.is_none() && item.instances.as_ref().is_none_or(|r| r.refused()) {
            return (None, 0);
        }
        item.instance_upload_pending = false;
        item.retained_gpu_evicted = false;
        item.resident_schema = item.retained_schema;
        if let Some(call) = item.kind.draw_call_mut() {
            call.instance_dirty = false;
        }
        let Some(decision) = decision else { return (None, 0) };
        let (held, written) = match decision {
            RegionUse::Reuse(held) => (held, 0),
            RegionUse::Write { held, byte_off, bytes } => {
                self.apply_slab_events(budget);
                let rows = &item.instances.as_ref().unwrap()[..draw.count * draw.slots];
                self.write_region(held, byte_off, bytes, vec![WasmPtrU8::new(f32_bytes(rows))]);
                (held, bytes)
            }
        };
        self.pin(held, draw.serial);
        (Some(held), written)
    }
}

impl Cx {
    /// The instance store's once-per-repaint close: spares age, a mostly dead slab starts
    /// evacuating, and an evacuated one drops what stayed behind.
    pub(crate) fn webgl_instance_frame_end(&mut self) {
        let submitted = self.frame_submission_serial();
        let completed = self.textures.1.serials.completed.load(std::sync::atomic::Ordering::Acquire);
        self.os.regions.store.end_frame(submitted, completed);
        self.os.apply_slab_events(&mut self.draw_lists.1.allocations);
    }

    fn webgl_instance_user(&mut self, list: usize, index: usize) -> Option<&mut crate::draw_list::CxDrawItem> {
        if list >= self.draw_lists.0.pool.len() || self.draw_lists.is_slot_freed(list) {
            return None;
        }
        let items = &mut self.draw_lists.0.pool[list].item.draw_items;
        (index < items.len()).then(|| items.binding_mut(index))
    }

    /// The page could not create or fill instance storage (`ToWasmInstanceUploadFailed`).
    /// A reply for a backing that is no longer live is ignored.
    pub(crate) fn webgl_instance_upload_failed(&mut self, failed: ToWasmInstanceUploadFailed) {
        let live = self.os.slab_backings.get(failed.slab_slot as usize).map_or(0, |b| b.generation);
        if live == 0 || live != failed.backing_generation {
            return;
        }
        if failed.scope == 1 {
            let Some(region) = failed.region else { return };
            let token = RegionRef { index: region.index, gen: region.gen };
            let key = self.os.regions.inst.iter().find(|(_, rec)| rec.held.region == token).map(|(key, _)| *key);
            if let Some(key) = key {
                // A publication prefix: final for that content, for every user of it.
                let users: Vec<(usize, usize)> = self
                    .os
                    .regions
                    .inst_items
                    .iter()
                    .filter(|(_, (_, used))| *used == key)
                    .map(|(user, _)| *user)
                    .collect();
                for (list, index) in users {
                    self.os.regions.release_publication((list, index));
                    if let Some(item) = self.webgl_instance_user(list, index) {
                        item.os.retained_failed = key.0;
                        item.instance_upload_pending = false;
                    }
                }
                crate::error!("WebGL refused the instance rows of publication {}; that content stays off screen until it changes", key.0);
            } else {
                // A recording's rows: written again on the item's next draw.
                for slot in &mut self.draw_lists.0.pool {
                    for index in 0..slot.item.draw_items.len() {
                        let item = slot.item.draw_items.binding_mut(index);
                        if item.instance_region.is_some_and(|held| held.region == token) {
                            self.os.regions.release_plain(item);
                            if let Some(call) = item.kind.draw_call_mut() {
                                call.instance_dirty = true;
                            }
                        }
                    }
                }
                crate::error!("WebGL refused a recording's instance rows; they are written again on its next draw");
            }
            return;
        }
        // The backing or its texture mirror is gone: as for a native slab eviction, its
        // regions are written again into a new backing on their users' next draw.
        let users = self.os.regions.evict_slab(failed.slab_slot);
        self.os.apply_slab_events(&mut self.draw_lists.1.allocations);
        for (list, index) in users {
            if let Some(item) = self.webgl_instance_user(list, index) {
                item.retained_gpu_evicted = true;
                if let Some(call) = item.kind.draw_call_mut() {
                    call.instance_dirty = true;
                }
            }
        }
        crate::error!(
            "WebGL instance slab {} lost its {}; its rows are written again",
            failed.slab_slot,
            if failed.scope == 2 { "texture mirror" } else { "backing" }
        );
        // One repaint per refusal; a refusal right after the repaint the last one asked for
        // waits for the app's next frame, so an allocation the browser keeps refusing never
        // turns into a repaint loop.
        if self.os.backing_refusal_repaint == 0 || self.os.backing_refusal_repaint + 1 < self.repaint_id {
            self.os.backing_refusal_repaint = self.repaint_id;
            for slot in &mut self.passes.0.pool {
                if slot.item.main_draw_list_id.is_some() {
                    slot.item.paint_dirty = true;
                }
            }
        }
    }
}

impl CxOsDrawShader {
    pub fn new(in_vertex: String, in_pixel: String, color_format: DrawShaderColorFormat, blend_op: DrawShaderBlendOp, textures: Vec<WTextureInput>) -> Self {
        let vertex = format!(
            "#version 300 es
#define VIEW_ID 0
precision highp float;
precision highp int;
precision highp sampler2D;
precision highp sampler2DShadow;
precision highp sampler2DArray;
vec4 sample2d(sampler2D sampler, vec2 pos){{return texture(sampler, vec2(pos.x, pos.y));}}
vec4 sample2d_lod(sampler2D sampler, vec2 pos, float lod){{return textureLod(sampler, vec2(pos.x, pos.y), lod);}}
vec4 samplecube(samplerCube sampler, vec3 dir){{return texture(sampler, dir);}}
vec4 samplecube_lod(samplerCube sampler, vec3 dir, float lod){{return textureLod(sampler, dir, lod);}}
vec4 depth_clip(vec4 w, vec4 c, float clip){{return c;}}
{}",
            in_vertex
        );

        let pixel = format!(
            "#version 300 es
#define VIEW_ID 0
precision highp float;
precision highp int;
precision highp sampler2D;
precision highp sampler2DShadow;
precision highp sampler2DArray;
vec4 sample2d(sampler2D sampler, vec2 pos){{return texture(sampler, vec2(pos.x, pos.y));}}
vec4 sample2d_lod(sampler2D sampler, vec2 pos, float lod){{return textureLod(sampler, vec2(pos.x, pos.y), lod);}}
vec4 samplecube(samplerCube sampler, vec3 dir){{return texture(sampler, dir);}}
vec4 samplecube_lod(samplerCube sampler, vec3 dir, float lod){{return textureLod(sampler, dir, lod);}}
vec4 depth_clip(vec4 w, vec4 c, float clip){{return c;}}
{}",
            in_pixel
        );

        Self {
            textures,
            in_vertex,
            in_pixel,
            vertex,
            pixel,
            color_format,
            blend_op,
        }
    }
}

#[derive(Default, Clone, Debug)]
pub struct CxOsPass {}

#[derive(Clone, Default)]
pub struct CxOsDrawList {}

/// The slab region a VAO's instance attributes read: the page's backing (slot and
/// generation) and the region in it.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct WebInstanceBinding {
    pub slab: u32,
    pub generation: u32,
    pub region_index: u32,
    pub region_gen: u32,
    pub byte_off: u64,
    pub byte_len: u64,
}

#[derive(Default, Clone)]
pub struct CxOsDrawCallVao {
    pub vao_id: usize,
    pub shader_id: Option<usize>,
    pub inst: Option<WebInstanceBinding>,
    pub geom_vb_id: Option<usize>,
    pub geom_ib_id: Option<usize>,
    pub geom_vb_generation: u64,
    pub geom_ib_generation: u64,
}

#[derive(Default, Clone)]
pub struct CxOsDrawCall {
    pub vao: Option<CxOsDrawCallVao>,
    /// The publication whose region write the page refused (0 = none). That
    /// content is not drawn and not retried; a new publication is tried.
    pub retained_failed: u64,
}

#[derive(Clone)]
pub struct CxOsDrawShader {
    pub textures: Vec<WTextureInput>,
    pub color_format: DrawShaderColorFormat,
    pub blend_op: DrawShaderBlendOp,
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
            instance_slabs: Vec::new(),
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
