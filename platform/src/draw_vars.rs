use crate::{
    area::Area,
    cx::Cx,
    draw_shader::*,
    geometry::GeometryId,
    makepad_math::*,
    makepad_script::mod_shader::ShaderIoType,
    makepad_script::mod_shader::SHADER_IO_DYN_INSTANCE,
    makepad_script::mod_shader::SHADER_IO_DYN_UNIFORM,
    makepad_script::pod::ScriptPodVec,
    makepad_script::pod_heap,
    makepad_script::*,
    script::vm::*,
    texture::Texture,
    uniform_buffer::UniformBuffer,
};

#[cfg(not(all(target_arch = "wasm32", makepad_precompiled_shaders)))]
use crate::makepad_script::{
    shader_compile::CompiledShader,
    shader_backend::ShaderBackend,
};
#[cfg(target_arch = "wasm32")]
use crate::makepad_script::shader_reflection::DrawShaderDesc;
use crate::makepad_script::mod_shader::SHADER_IO_VERTEX_BUFFER;

/// The largest dyn-uniform block a shader may declare, in f32 lanes: 16 KB,
/// the uniform-block size every backend takes (GL/WebGL 2's guaranteed
/// minimum; Metal, D3D11 and WGSL take more). A block is as big as its
/// shader's uniforms (reflection), not this.
pub const DRAW_CALL_MAX_UNIFORM_FLOATS: usize = 4096;
// Combined bindings, not the fragment-stage limit. Full rigid PBR uses
// 16 fragment textures plus one vertex-only morph texture with GI enabled.
// WebGL2 guarantees 16 per stage and at least 32 combined units.
pub const DRAW_CALL_TEXTURE_SLOTS: usize = 17;
pub const DRAW_CALL_UNIFORM_BUFFER_SLOTS: usize = 2;
pub const DRAW_CALL_DYN_INSTANCES: usize = 32;

/// f32 lanes placed before `dyn_instances` so the array ends exactly where
/// `DrawVars` ends. The instance data a draw call uploads is the tail of
/// `dyn_instances` followed directly by the `#[live]` fields of the shader
/// struct that embeds `DrawVars` (`as_slice`), so any padding after the array
/// shifts every one of those fields by a lane. On wasm32 (4-byte pointers and
/// usize) the 8-byte-aligned struct ended 4 bytes past the array: the shader
/// read `char_index` where `texture_index` was, and web text drew as boxes.
/// With the fields before it as they are, no target needs a lane (the
/// assert below names a layout change that does).
const DRAW_VARS_TAIL_PAD: usize = 0;

#[derive(Clone, Script, Debug)]
#[repr(C)]
pub struct DrawVars {
    #[rust]
    pub area: Area,
    #[rust]
    pub dyn_instance_start: usize,
    #[rust]
    pub dyn_instance_slots: usize,
    #[rust]
    pub options: CxDrawShaderOptions,
    #[rust]
    pub append_group_id: u64,
    #[rust]
    pub draw_shader_id: Option<DrawShaderId>,
    #[rust]
    pub geometry_id: Option<GeometryId>,
    /// The shader's dyn-uniform block, `mapping.dyn_uniforms.total_slots`
    /// lanes (sized when the shader is bound).
    #[rust]
    pub dyn_uniforms: Vec<f32>,
    #[rust]
    pub texture_slots: [Option<Texture>; DRAW_CALL_TEXTURE_SLOTS],
    #[rust]
    pub uniform_buffer_slots: [Option<UniformBuffer>; DRAW_CALL_UNIFORM_BUFFER_SLOTS],
    #[rust([0f32; DRAW_VARS_TAIL_PAD])]
    pub dyn_instances_pad: [f32; DRAW_VARS_TAIL_PAD],
    #[rust([0f32; DRAW_CALL_DYN_INSTANCES])]
    pub dyn_instances: [f32; DRAW_CALL_DYN_INSTANCES],
}

// See DRAW_VARS_TAIL_PAD: the instance fields must follow `dyn_instances`
// with no gap, on every target.
const _: () = assert!(
    core::mem::offset_of!(DrawVars, dyn_instances)
        + core::mem::size_of::<[f32; DRAW_CALL_DYN_INSTANCES]>()
        == core::mem::size_of::<DrawVars>(),
    "DrawVars must end with dyn_instances (adjust DRAW_VARS_TAIL_PAD)"
);

impl ScriptHook for DrawVars {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        DrawVars::prune_stale_object_shader_cache(vm);

        if !apply.is_default() && !apply.is_animate() {
            #[cfg(not(target_arch = "wasm32"))]
            if vm.host.cx().draw_shaders.packs.is_recording() {
                if let Some(io_self) = value.as_object() {
                    DrawVars::record_shader_pack_entry(vm, io_self);
                }
            }
            self.compile_shader(vm, apply, value);
        }

        // Read draw-call level options from the shader object.
        if let Some(io_self) = value.as_object() {
            let group_value = vm.value(io_self, id!(draw_call_group));
            if let Some(id) = group_value.as_id() {
                self.options.draw_call_group = id;
            } else if let Some(v) = group_value.as_f64() {
                self.options.draw_call_group = LiveId(v as u64);
            }

            let depth_write_value = vm.value(io_self, id!(depth_write));
            if let Some(v) = depth_write_value.as_bool() {
                self.options.depth_write = v;
            } else if let Some(v) = depth_write_value.as_f64() {
                self.options.depth_write = v != 0.0;
            }

            let alpha_blend_value = vm.value(io_self, id!(alpha_blend));
            if let Some(v) = alpha_blend_value.as_bool() {
                self.options.alpha_blend = v;
            } else if let Some(v) = alpha_blend_value.as_f64() {
                self.options.alpha_blend = v != 0.0;
            }

            let backface_culling_value =
                vm.value(io_self, id!(backface_culling));
            if let Some(v) = backface_culling_value.as_bool() {
                self.options.backface_culling = v;
            } else if let Some(v) = backface_culling_value.as_f64() {
                self.options.backface_culling = v != 0.0;
            }
        }
        // lets fill our values
        if self.draw_shader_id.is_some() {
            if let Some(io_self) = value.as_object() {
                let (mut host, heap) = vm.host_heap();
                let cx = host.cx_mut();
                // For eval applies, only update values that exist at the top level (shallow)
                // This avoids re-reading all values from the prototype chain
                let shallow = apply.is_eval();
                self.fill_dyn_instances(cx, heap, io_self, shallow);
                self.fill_dyn_uniforms(cx, heap, io_self, shallow);
            }
        }
        // Update areas for animated properties only
        if apply.is_animate() || apply.is_eval() {
            if let Some(io_self) = value.as_object() {
                let (mut host, heap) = vm.host_heap();
                let cx = host.cx_mut();
                self.update_instance_areas_when_in_object(cx, heap, io_self);
                self.update_uniform_areas_when_in_object(cx, heap, io_self);
            }
        }
    }
}

impl DrawVars {
    /// Loud report for a failed draw-shader compile. A shader that fails
    /// must never fall back silently: the draw is skipped from then on and
    /// the only on-screen symptom is a flat clear-color region where the
    /// shader should have painted. Names the shader and lists every
    /// collected compile error.
    pub fn log_shader_compile_failure(
        vm: &ScriptVm,
        io_self: ScriptObject,
        output: &crate::makepad_script::shader_compile::CompiledShader,
    ) {
        let name = vm.heap()
            .object_type_name_in_chain(io_self)
            .map(|id| format!("{}", id))
            .unwrap_or_else(|| format!("<script object {}>", io_self.index()));
        let report = output.error_report();
        crate::shader_error::note(format!("{name}: {report}"));
        crate::error!(
            "draw shader '{}' failed to compile and will NOT be drawn:\n{}",
            name,
            report
        );
    }

    fn prune_stale_object_shader_cache(vm: &mut ScriptVm) {
        let object_reuse_epoch = vm.heap().object_reuse_epoch();
        let cx = vm.host.cx_mut();
        if cx.draw_shaders.cache_object_reuse_epoch_seen != object_reuse_epoch {
            cx.draw_shaders.cache_object_id_to_shader.clear();
            cx.draw_shaders.cache_object_reuse_epoch_seen = object_reuse_epoch;
        }
    }

    pub fn set_texture(&mut self, slot: usize, texture: &Texture) {
        self.texture_slots[slot] = Some(texture.clone());
    }

    pub fn empty_texture(&mut self, slot: usize) {
        self.texture_slots[slot] = None;
    }

    pub fn set_uniform_buffer(&mut self, slot: usize, uniform_buffer: &UniformBuffer) {
        self.uniform_buffer_slots[slot] = Some(uniform_buffer.clone());
    }

    pub fn empty_uniform_buffer(&mut self, slot: usize) {
        self.uniform_buffer_slots[slot] = None;
    }

    #[track_caller]
    pub fn redraw(&self, cx: &mut Cx) {
        self.area.redraw(cx);
    }

    pub fn area(&self) -> Area {
        self.area
    }

    pub fn can_instance(&self) -> bool {
        self.draw_shader_id.is_some()
    }

    pub fn as_slice<'a>(&'a self) -> &'a [f32] {
        debug_assert!(
            self.dyn_instance_start <= self.dyn_instances.len(),
            "dyn instance start out of bounds: start={} len={}",
            self.dyn_instance_start,
            self.dyn_instances.len()
        );
        unsafe {
            // NOTE:
            // Instance layout intentionally spans beyond `dyn_instances` into trailing
            // live instance fields in repr(C) draw shader structs.
            std::slice::from_raw_parts(
                self.dyn_instances
                    .as_ptr()
                    .add(self.dyn_instance_start)
                    .cast::<f32>(),
                self.dyn_instance_slots,
            )
        }
    }

    /// Update instance areas only for inputs that exist on the given script object.
    /// Used during animation to only update animated properties.
    fn update_instance_areas_when_in_object(
        &mut self,
        cx: &mut Cx,
        heap: &ScriptHeap,
        io_self: ScriptObject,
    ) {
        if let Some(draw_shader_id) = self.draw_shader_id {
            if let Some(inst) = self.area.valid_instance(cx) {
                let sh = &cx.draw_shaders[draw_shader_id.index];
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                let draw_item = &mut draw_list.draw_items[inst.draw_item_id];
                // A stale area: the item its instance points at has since become
                // a sub-list or a text run. Nothing to write into; the next
                // draw re-emits the call with the values this `DrawVars` holds.
                let Some(draw_call) = draw_item.kind.draw_call_mut() else {
                    return;
                };

                // A call of another shader (the shader changed under this area, e.g. a theme
                // swap): its slot layout is not the one this `DrawVars` writes. Stale too.
                if draw_call.draw_shader_id != draw_shader_id {
                    return;
                }
                let repeat = inst.instance_count;
                let stride = sh.mapping.instances.total_slots;
                let instances = &mut draw_item.instances.as_mut().unwrap()[inst.instance_offset..];
                let inst_slice = self.as_slice();
                let mut any_updated = false;

                for input in &sh.mapping.instances.inputs {
                    let key: ScriptValue = input.id.into();
                    if heap.map_own_value(io_self, key).is_some() {
                        for j in 0..repeat {
                            for i in 0..input.slots {
                                instances[input.offset + i + j * stride] =
                                    inst_slice[input.offset + i]
                            }
                        }
                        any_updated = true;
                    }
                }

                if any_updated {
                    draw_call.instance_dirty = true;
                    if let Some(pass_id) = draw_list.draw_pass_id {
                        cx.passes[pass_id].paint_dirty = true;
                    }
                }
            }
        }
    }

    /// Update uniform areas only for inputs that exist on the given script object.
    /// Used during animation to only update animated properties.
    fn update_uniform_areas_when_in_object(
        &mut self,
        cx: &mut Cx,
        heap: &ScriptHeap,
        io_self: ScriptObject,
    ) {
        if let Some(draw_shader_id) = self.draw_shader_id {
            if let Some(inst) = self.area.valid_instance(cx) {
                let uniforms_gen = cx.next_uniform_gen();
                let sh = &cx.draw_shaders[draw_shader_id.index];
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                let draw_item = &mut draw_list.draw_items[inst.draw_item_id];
                // A stale area: the item its instance points at has since become
                // a sub-list or a text run. Nothing to write into; the next
                // draw re-emits the call with the values this `DrawVars` holds.
                let Some(draw_call) = draw_item.kind.draw_call_mut() else {
                    return;
                };

                // A call of another shader (the shader changed under this area, e.g. a theme
                // swap): its slot layout is not the one this `DrawVars` writes. Stale too.
                if draw_call.draw_shader_id != draw_shader_id {
                    return;
                }
                let mut any_updated = false;

                for input in &sh.mapping.dyn_uniforms.inputs {
                    let key: ScriptValue = input.id.into();
                    if heap.map_own_value(io_self, key).is_some() {
                        for i in 0..input.slots {
                            draw_call.dyn_uniforms[input.offset + i] =
                                self.dyn_uniforms.get(input.offset + i).copied().unwrap_or(0.0)
                        }
                        any_updated = true;
                    }
                }

                if any_updated {
                    draw_call.mark_uniforms_dirty(uniforms_gen);
                    if let Some(pass_id) = draw_list.draw_pass_id {
                        cx.passes[pass_id].paint_dirty = true;
                    }
                    self.area.redraw(cx);
                }
            }
        }
    }

    pub fn update_rect(&mut self, cx: &mut Cx, rect: Rect) {
        if let Some(draw_shader_id) = self.draw_shader_id {
            if let Some(inst) = self.area.valid_instance(cx) {
                let sh = &cx.draw_shaders[draw_shader_id.index];
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                let draw_item = &mut draw_list.draw_items[inst.draw_item_id];
                // A stale area: the item its instance points at has since become
                // a sub-list or a text run. Nothing to write into; the next
                // draw re-emits the call with the values this `DrawVars` holds.
                let Some(draw_call) = draw_item.kind.draw_call_mut() else {
                    return;
                };

                // A call of another shader (the shader changed under this area, e.g. a theme
                // swap): its slot layout is not the one this `DrawVars` writes. Stale too.
                if draw_call.draw_shader_id != draw_shader_id {
                    return;
                }

                let repeat = inst.instance_count;
                let stride = sh.mapping.instances.total_slots;
                let instances = &mut draw_item.instances.as_mut().unwrap()[inst.instance_offset..];

                for input in &sh.mapping.instances.inputs {
                    if input.id == live_id!(rect_pos) {
                        for j in 0..repeat {
                            instances[input.offset + 0 + j * stride] = rect.pos.x as f32;
                            instances[input.offset + 1 + j * stride] = rect.pos.y as f32;
                        }
                    }
                    if input.id == live_id!(rect_size) {
                        for j in 0..repeat {
                            instances[input.offset + 0 + j * stride] = rect.size.x as f32;
                            instances[input.offset + 1 + j * stride] = rect.size.y as f32;
                        }
                    }
                }
                draw_call.instance_dirty = true;
                if let Some(pass_id) = draw_list.draw_pass_id {
                        cx.passes[pass_id].paint_dirty = true;
                    }
            }
        }
    }

    pub fn update_instance_area_value(&mut self, cx: &mut Cx, id: &[LiveId]) {
        if let Some(draw_shader_id) = self.draw_shader_id {
            if let Some(inst) = self.area.valid_instance(cx) {
                let sh = &cx.draw_shaders[draw_shader_id.index];
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                let draw_item = &mut draw_list.draw_items[inst.draw_item_id];
                // A stale area: the item its instance points at has since become
                // a sub-list or a text run. Nothing to write into; the next
                // draw re-emits the call with the values this `DrawVars` holds.
                let Some(draw_call) = draw_item.kind.draw_call_mut() else {
                    return;
                };

                // A call of another shader (the shader changed under this area, e.g. a theme
                // swap): its slot layout is not the one this `DrawVars` writes. Stale too.
                if draw_call.draw_shader_id != draw_shader_id {
                    return;
                }

                let repeat = inst.instance_count;
                let stride = sh.mapping.instances.total_slots;
                let instances = &mut draw_item.instances.as_mut().unwrap()[inst.instance_offset..];
                let slice = self.as_slice();
                for input in &sh.mapping.instances.inputs {
                    if input.id == id[0] {
                        for j in 0..repeat {
                            for k in 0..input.slots {
                                instances[input.offset + k + j * stride] = slice[input.offset + k];
                            }
                        }
                    }
                }
                draw_call.instance_dirty = true;
                if let Some(pass_id) = draw_list.draw_pass_id {
                        cx.passes[pass_id].paint_dirty = true;
                    }
            }
        }
    }

    pub fn get_instance(&self, cx: &mut Cx, inst: LiveId, value: &mut [f32]) {
        if let Some(draw_shader_id) = self.draw_shader_id {
            let sh = &cx.draw_shaders[draw_shader_id.index];
            let self_slice = self.as_slice();
            for input in &sh.mapping.instances.inputs {
                let offset = input.offset;
                let slots = input.slots;
                if input.id == inst {
                    for i in 0..value.len().min(slots) {
                        value[i] = self_slice[offset + i]
                    }
                }
            }
        }
    }

    /// Read an instance value from the currently bound draw area (first instance).
    /// Returns `true` when the instance input exists and a value was read.
    pub fn get_instance_on_area(&self, cx: &Cx, inst: LiveId, value: &mut [f32]) -> bool {
        let Some(draw_shader_id) = self.draw_shader_id else {
            return false;
        };
        let Some(area) = self.area.valid_instance(cx) else {
            return false;
        };

        let sh = &cx.draw_shaders[draw_shader_id.index];
        let Some(input) = sh
            .mapping
            .instances
            .inputs
            .iter()
            .find(|input| input.id == inst)
        else {
            return false;
        };
        let Some(draw_list) = cx.draw_lists.checked_index(area.draw_list_id) else {
            return false;
        };
        let Some(draw_item) = draw_list.draw_items.buffer.get(area.draw_item_id) else {
            return false;
        };
        let Some(instances) = draw_item.instances.as_ref() else {
            return false;
        };

        let stride = sh.mapping.instances.total_slots;
        let available = instances.len().saturating_sub(area.instance_offset);
        let max_count = if stride == 0 { 0 } else { available / stride };
        if area.instance_count == 0 || area.instance_count > max_count {
            return false;
        }

        let base = area.instance_offset + input.offset;
        if base + input.slots > instances.len() {
            return false;
        }
        for i in 0..value.len().min(input.slots) {
            value[i] = instances[base + i];
        }
        true
    }

    /// Set a script-declared instance field (`name: instance(..)`) by name
    /// for the next draw. False when the shader has no such field.
    pub fn set_dyn_instance(&mut self, cx: &Cx, instance: LiveId, value: &[f32]) -> bool {
        let mut found = false;
        if let Some(draw_shader_id) = self.draw_shader_id {
            let sh = &cx.draw_shaders[draw_shader_id.index];
            for input in &sh.mapping.dyn_instances.inputs {
                let offset = (self.dyn_instances.len() - sh.mapping.dyn_instances.total_slots)
                    + input.offset;
                let slots = input.slots;
                if input.id == instance {
                    for i in 0..value.len().min(slots) {
                        self.dyn_instances[offset + i] = value[i];
                    }
                    found = true;
                }
            }
        }
        found
    }

    /// Grow the dyn-uniform block to `floats` lanes (a shader bound, or
    /// swapped in by its id, has that many).
    pub fn fit_dyn_uniforms(&mut self, floats: usize) {
        if self.dyn_uniforms.len() < floats {
            self.dyn_uniforms.resize(floats, 0.0);
        }
    }

    pub fn get_uniform(&self, cx: &mut Cx, uniform: LiveId, value: &mut [f32]) {
        if let Some(draw_shader_id) = self.draw_shader_id {
            let sh = &cx.draw_shaders[draw_shader_id.index];
            for input in &sh.mapping.dyn_uniforms.inputs {
                let offset = input.offset;
                let slots = input.slots;
                if input.id == uniform {
                    for i in 0..value.len().min(slots) {
                        value[i] = self.dyn_uniforms.get(offset + i).copied().unwrap_or(0.0);
                    }
                }
            }
        }
    }

    pub fn set_uniform(&mut self, cx: &Cx, uniform: LiveId, value: &[f32]) {
        if let Some(draw_shader_id) = self.draw_shader_id {
            let sh = &cx.draw_shaders[draw_shader_id.index];
            self.fit_dyn_uniforms(sh.mapping.dyn_uniforms.total_slots);
            for input in &sh.mapping.dyn_uniforms.inputs {
                let offset = input.offset;
                let slots = input.slots;
                if input.id == uniform {
                    for i in 0..value.len().min(slots) {
                        self.dyn_uniforms[offset + i] = value[i]
                    }
                }
            }
        }
    }

    /// The dyn-uniform block range `(offset, slots)` of one uniform of this
    /// shader, for callers that patch a retained draw call in place.
    pub fn uniform_range(&self, cx: &Cx, uniform: LiveId) -> Option<(usize, usize)> {
        let draw_shader_id = self.draw_shader_id?;
        let sh = &cx.draw_shaders[draw_shader_id.index];
        sh.mapping.dyn_uniforms.inputs.iter().find(|input| input.id == uniform).map(|input| (input.offset, input.slots))
    }

    /// Sets a uniform value and also updates the draw call on the area if valid.
    /// This is used to update uniforms after drawing has completed.
    pub fn set_uniform_on_area(&mut self, cx: &mut Cx, id: LiveId, value: &[f32]) {
        if let Some(draw_shader_id) = self.draw_shader_id {
            let sh = &cx.draw_shaders[draw_shader_id.index];
            self.fit_dyn_uniforms(sh.mapping.dyn_uniforms.total_slots);

            // Find the uniform input
            if let Some(input) = sh.mapping.dyn_uniforms.inputs.iter().find(|i| i.id == id) {
                let slots = input.slots.min(value.len());
                let offset = input.offset;

                // Update local dyn_uniforms
                for i in 0..slots {
                    self.dyn_uniforms[offset + i] = value[i];
                }

                // Update the draw call if we have a valid area
                if let Some(inst) = self.area.valid_instance(cx) {
                    let uniforms_gen = cx.next_uniform_gen();
                    let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                    let draw_item = &mut draw_list.draw_items[inst.draw_item_id];
                    // A stale area: the item its instance points at has since become
                // a sub-list or a text run. Nothing to write into; the next
                // draw re-emits the call with the values this `DrawVars` holds.
                let Some(draw_call) = draw_item.kind.draw_call_mut() else {
                    return;
                };

                // A call of another shader (the shader changed under this area, e.g. a theme
                // swap): its slot layout is not the one this `DrawVars` writes. Stale too.
                if draw_call.draw_shader_id != draw_shader_id {
                    return;
                }

                    for i in 0..slots {
                        draw_call.dyn_uniforms[offset + i] = value[i];
                    }
                    draw_call.mark_uniforms_dirty(uniforms_gen);
                    if let Some(pass_id) = draw_list.draw_pass_id {
                        cx.passes[pass_id].paint_dirty = true;
                    }
                }
            }
        }
    }

    /// Pushes every dyn uniform and texture slot this `DrawVars` holds onto
    /// the retained draw call behind `area`, and marks its pass for repaint.
    /// The whole-block twin of `set_uniform_on_area`: a widget that keeps
    /// its draw lists across frames re-stamps its staging copy per call
    /// (`set_uniform`, `set_texture`) and hands the result over in one
    /// copy, so a camera move touches uniforms only and never the resident
    /// instance buffers. `false` when the area is stale or belongs to
    /// another shader; nothing is written then.
    pub fn update_uniforms_on_area(&self, cx: &mut Cx, area: Area) -> bool {
        let Some(draw_shader_id) = self.draw_shader_id else {
            return false;
        };
        let Some(inst) = area.valid_instance(cx).copied() else {
            return false;
        };
        let uniforms_gen = cx.next_uniform_gen();
        let draw_list = &mut cx.draw_lists[inst.draw_list_id];
        let draw_item = &mut draw_list.draw_items[inst.draw_item_id];
        let Some(draw_call) = draw_item.kind.draw_call_mut() else {
            return false;
        };
        if draw_call.draw_shader_id != draw_shader_id {
            return false;
        }
        crate::draw_list::copy_dyn_uniforms(&mut draw_call.dyn_uniforms, &self.dyn_uniforms);
        draw_call.texture_slots = self.texture_slots.clone();
        draw_call.uniform_buffer_slots = self.uniform_buffer_slots.clone();
        draw_call.mark_uniforms_dirty(uniforms_gen);
        if let Some(pass_id) = draw_list.draw_pass_id {
            cx.passes[pass_id].paint_dirty = true;
        }
        true
    }

    /// Pushes every dyn uniform and texture slot this `DrawVars` holds onto
    /// EVERY retained draw call of this shader in the draw list `list`, and
    /// marks its pass for repaint: the list-wide twin of
    /// `update_uniforms_on_area`, for a batch a widget recorded once and
    /// re-presents under this frame's camera. `false` when no call took it.
    pub fn update_uniforms_on_draw_list(
        &self,
        cx: &mut Cx,
        list: crate::draw_list::DrawListId,
    ) -> bool {
        let Some(draw_shader_id) = self.draw_shader_id else {
            return false;
        };
        let Some(draw_list) = cx.draw_lists.checked_index(list) else {
            return false;
        };
        let pass_id = draw_list.draw_pass_id;
        let uniform_gen = &mut cx.uniform_gen;
        let draw_list = &mut cx.draw_lists[list];
        let mut touched = false;
        for item in 0..draw_list.draw_items.len() {
            let Some(draw_call) = draw_list.draw_items[item].kind.draw_call_mut() else {
                continue;
            };
            if draw_call.draw_shader_id != draw_shader_id {
                continue;
            }
            crate::draw_list::copy_dyn_uniforms(&mut draw_call.dyn_uniforms, &self.dyn_uniforms);
            draw_call.texture_slots = self.texture_slots.clone();
            draw_call.uniform_buffer_slots = self.uniform_buffer_slots.clone();
            draw_call.mark_uniforms_dirty(Cx::next_uniform_gen_from(uniform_gen));
            touched = true;
        }
        if touched {
            if let Some(pass_id) = pass_id {
                cx.passes[pass_id].paint_dirty = true;
            }
        }
        touched
    }

    /// Writes one uniform into every retained draw call of this shader in
    /// `list`. Identical stored bits leave those calls and the pass untouched;
    /// a real change issues a new uniform generation and marks the pass dirty.
    /// A widget whose instances batch into many calls (texture changes split
    /// them) can move its camera between redraws with this: the standing
    /// buffers re-present under fresh uniforms — nothing is rebuilt.
    pub fn set_uniform_on_draw_list(&mut self, cx: &mut Cx, list: Area, id: LiveId, value: &[f32]) {
        let Some(draw_shader_id) = self.draw_shader_id else { return };
        let Some(draw_list_id) = list.draw_list_id() else { return };
        let sh = &cx.draw_shaders[draw_shader_id.index];
        self.fit_dyn_uniforms(sh.mapping.dyn_uniforms.total_slots);
        let Some(input) = sh.mapping.dyn_uniforms.inputs.iter().find(|i| i.id == id) else { return };
        let slots = input.slots.min(value.len());
        let offset = input.offset;
        for i in 0..slots {
            self.dyn_uniforms[offset + i] = value[i];
        }
        let uniform_gen = &mut cx.uniform_gen;
        let draw_list = &mut cx.draw_lists[draw_list_id];
        let mut touched = false;
        for item in 0..draw_list.draw_items.len() {
            if draw_list.draw_items.set_dyn_uniform(
                item,
                draw_shader_id,
                offset,
                &value[..slots],
                uniform_gen,
            ) {
                touched = true;
            }
        }
        if touched {
            if let Some(pass_id) = draw_list.draw_pass_id {
                cx.passes[pass_id].paint_dirty = true;
            }
        }
    }

    /// Sets an instance value on all instances in the area.
    /// This is used to update instance data after drawing has completed.
    pub fn set_instance_on_area(&mut self, cx: &mut Cx, id: LiveId, value: &[f32]) {
        if let Some(draw_shader_id) = self.draw_shader_id {
            if let Some(inst) = self.area.valid_instance(cx) {
                let sh = &cx.draw_shaders[draw_shader_id.index];

                // Find the instance input
                if let Some(input) = sh.mapping.instances.inputs.iter().find(|i| i.id == id) {
                    let slots = input.slots.min(value.len());
                    let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                    let draw_item = &mut draw_list.draw_items[inst.draw_item_id];
                    // A stale area: the item its instance points at has since become
                // a sub-list or a text run. Nothing to write into; the next
                // draw re-emits the call with the values this `DrawVars` holds.
                let Some(draw_call) = draw_item.kind.draw_call_mut() else {
                    return;
                };

                // A call of another shader (the shader changed under this area, e.g. a theme
                // swap): its slot layout is not the one this `DrawVars` writes. Stale too.
                if draw_call.draw_shader_id != draw_shader_id {
                    return;
                }

                    let stride = sh.mapping.instances.total_slots;
                    let all_instances = draw_item.instances.as_mut().unwrap();

                    // Validate area bounds
                    let available = all_instances.len().saturating_sub(inst.instance_offset);
                    let max_count = available / stride;
                    if inst.instance_count > max_count {
                        crate::log!(
                            "stale: cnt={} max={} redraw={} list_redraw={}",
                            inst.instance_count,
                            max_count,
                            inst.redraw_id,
                            draw_list.redraw_id
                        );
                        return; // Area is stale, skip update
                    }

                    let instances = &mut all_instances[inst.instance_offset..];

                    // Update all instances in this area
                    for j in 0..inst.instance_count {
                        for i in 0..slots {
                            instances[input.offset + i + j * stride] = value[i];
                        }
                    }

                    draw_call.instance_dirty = true;
                    if let Some(pass_id) = draw_list.draw_pass_id {
                        cx.passes[pass_id].paint_dirty = true;
                    }
                }
            }
        }
    }

    fn fill_dyn_instances(
        &mut self,
        cx: &Cx,
        heap: &ScriptHeap,
        io_self: ScriptObject,
        shallow: bool,
    ) {
        if let Some(draw_shader_id) = self.draw_shader_id {
            let mapping = &cx.draw_shaders.shaders[draw_shader_id.index].mapping;
            let base_offset = self.dyn_instances.len() - mapping.dyn_instances.total_slots;

            for input in &mapping.dyn_instances.inputs {
                let slot_offset = base_offset + input.offset;
                let value = Self::extract_shader_io_value(
                    heap,
                    io_self,
                    input.id,
                    SHADER_IO_DYN_INSTANCE,
                    shallow,
                );
                let wrote_slot = !value.is_nil() && !value.is_err();
                if wrote_slot {
                    Self::write_value_to_f32_slots(
                        heap,
                        value,
                        &mut self.dyn_instances,
                        slot_offset,
                        input.slots,
                        input.attr_format,
                    );
                }
            }
        }
    }

    fn fill_dyn_uniforms(
        &mut self,
        cx: &Cx,
        heap: &ScriptHeap,
        io_self: ScriptObject,
        shallow: bool,
    ) {
        if let Some(draw_shader_id) = self.draw_shader_id {
            let mapping = &cx.draw_shaders.shaders[draw_shader_id.index].mapping;
            self.fit_dyn_uniforms(mapping.dyn_uniforms.total_slots);

            for input in &mapping.dyn_uniforms.inputs {
                let value = Self::extract_shader_io_value(
                    heap,
                    io_self,
                    input.id,
                    SHADER_IO_DYN_UNIFORM,
                    shallow,
                );
                if !value.is_nil() && !value.is_err() {
                    Self::write_value_to_f32_slots(
                        heap,
                        value,
                        &mut self.dyn_uniforms,
                        input.offset,
                        input.slots,
                        DrawShaderAttrFormat::Float,
                    );
                }
            }
        }
    }

    fn extract_shader_io_value(
        heap: &ScriptHeap,
        io_self: ScriptObject,
        id: LiveId,
        expected_io_type: ShaderIoType,
        shallow: bool,
    ) -> ScriptValue {
        // For shallow lookups, only check the object's own map (no prototype chain)
        let value = if shallow {
            heap.map_own_value(io_self, id.into()).unwrap_or(NIL)
        } else {
            heap.value(io_self, id.into(), NoTrap)
        };

        // Check if it's a shader IO object with the expected type
        if let Some(value_obj) = value.as_object() {
            if let Some(io_type) = heap.as_shader_io(value_obj) {
                if io_type == expected_io_type {
                    // The value is stored as the prototype
                    return heap.proto(value_obj);
                }
            }
        }

        // Return the value directly
        value
    }

    /// Write a ScriptValue to f32 slots in an output array at the given offset.
    pub fn write_value_to_f32_slots(
        heap: &ScriptHeap,
        value: ScriptValue,
        output: &mut [f32],
        offset: usize,
        slots: usize,
        attr_format: DrawShaderAttrFormat,
    ) {
        // Try f64 first (most common for abstract numbers)
        if let Some(v) = value.as_f64() {
            let v = match attr_format.f32_write_kind() {
                DrawShaderF32WriteKind::Float => v as f32,
                DrawShaderF32WriteKind::UInt => f32::from_bits(v as u32),
                DrawShaderF32WriteKind::SInt => f32::from_bits(v as i32 as u32),
            };
            for i in 0..slots {
                output[offset + i] = v;
            }
            return;
        }

        // Try u40 (common integer format in script)
        if let Some(v) = value.as_u40() {
            let v = match attr_format.f32_write_kind() {
                DrawShaderF32WriteKind::Float => v as f32,
                DrawShaderF32WriteKind::UInt => f32::from_bits(v as u32),
                DrawShaderF32WriteKind::SInt => f32::from_bits(v as i32 as u32),
            };
            for i in 0..slots {
                output[offset + i] = v;
            }
            return;
        }

        // Try f32
        if let Some(v) = value.as_f32() {
            let v = match attr_format.f32_write_kind() {
                DrawShaderF32WriteKind::Float => v,
                DrawShaderF32WriteKind::UInt => f32::from_bits(v as u32),
                DrawShaderF32WriteKind::SInt => f32::from_bits(v as i32 as u32),
            };
            for i in 0..slots {
                output[offset + i] = v;
            }
            return;
        }

        // Try f16
        if let Some(v) = value.as_f16() {
            let v = match attr_format.f32_write_kind() {
                DrawShaderF32WriteKind::Float => v,
                DrawShaderF32WriteKind::UInt => f32::from_bits(v as u32),
                DrawShaderF32WriteKind::SInt => f32::from_bits(v as i32 as u32),
            };
            for i in 0..slots {
                output[offset + i] = v;
            }
            return;
        }

        // Try u32/i32
        if let Some(v) = value.as_u32() {
            let v = match attr_format.f32_write_kind() {
                DrawShaderF32WriteKind::Float => v as f32,
                DrawShaderF32WriteKind::UInt | DrawShaderF32WriteKind::SInt => f32::from_bits(v),
            };
            for i in 0..slots {
                output[offset + i] = v;
            }
            return;
        }
        if let Some(v) = value.as_i32() {
            let v = match attr_format.f32_write_kind() {
                DrawShaderF32WriteKind::Float => v as f32,
                DrawShaderF32WriteKind::UInt | DrawShaderF32WriteKind::SInt => {
                    f32::from_bits(v as u32)
                }
            };
            for i in 0..slots {
                output[offset + i] = v;
            }
            return;
        }

        // Try bool
        if let Some(v) = value.as_bool() {
            let v = match attr_format.f32_write_kind() {
                DrawShaderF32WriteKind::Float => {
                    if v {
                        1.0
                    } else {
                        0.0
                    }
                }
                DrawShaderF32WriteKind::UInt | DrawShaderF32WriteKind::SInt => {
                    f32::from_bits(if v { 1 } else { 0 })
                }
            };
            for i in 0..slots {
                output[offset + i] = v;
            }
            return;
        }

        // Try color (u32 RGBA)
        if let Some(c) = value.as_color() {
            let v = Vec4f::from_u32(c);
            if slots >= 1 {
                output[offset + 0] = v.x;
            }
            if slots >= 2 {
                output[offset + 1] = v.y;
            }
            if slots >= 3 {
                output[offset + 2] = v.z;
            }
            if slots >= 4 {
                output[offset + 3] = v.w;
            }
            return;
        }

        // An animator state's `snap(..)`: an object standing for the value
        // it carries. A state the animator plays is resolved before it gets
        // here, but a state applied as a widget's default (`hover: {default:
        // @down}`) is handed over as written, and a slot that skipped it
        // left the widget in the state before.
        if let Some(obj) = value.as_object() {
            if heap.has_apply_transform(value) {
                let carried = heap.value(obj, live_id!(value).into(), NoTrap);
                if !carried.is_nil() && !carried.is_err() && carried.as_object() != Some(obj) {
                    Self::write_value_to_f32_slots(heap, carried, output, offset, slots, attr_format);
                    return;
                }
            }
        }

        // Try repr(u32) enum variant objects used by script APIs.
        // These carry the numeric payload in `_repr_u32_enum_value`.
        if let Some(obj) = value.as_object() {
            let enum_value = heap.value(obj, live_id!(_repr_u32_enum_value).into(), NoTrap);
            if let Some(v) = enum_value.as_f64() {
                let v = match attr_format.f32_write_kind() {
                    DrawShaderF32WriteKind::Float => v as f32,
                    DrawShaderF32WriteKind::UInt => f32::from_bits(v as u32),
                    DrawShaderF32WriteKind::SInt => f32::from_bits(v as i32 as u32),
                };
                for i in 0..slots {
                    output[offset + i] = v;
                }
                return;
            }
            if let Some(v) = enum_value.as_u32() {
                let v = match attr_format.f32_write_kind() {
                    DrawShaderF32WriteKind::Float => v as f32,
                    DrawShaderF32WriteKind::UInt | DrawShaderF32WriteKind::SInt => f32::from_bits(v),
                };
                for i in 0..slots {
                    output[offset + i] = v;
                }
                return;
            }
        }

        // Try pod (Vec2f, Vec3f, Vec4f, etc.)
        if let Some(pod) = value.as_pod() {
            let (pod_type, data) = heap.pod_data(pod);

            match pod_type.ty().kind() {
                crate::makepad_script::pod::ScriptPodTyTag::F32 => {
                    let v = match attr_format.f32_write_kind() {
                        DrawShaderF32WriteKind::Float => f32::from_bits(data[0]),
                        DrawShaderF32WriteKind::UInt => {
                            f32::from_bits(f32::from_bits(data[0]) as u32)
                        }
                        DrawShaderF32WriteKind::SInt => {
                            f32::from_bits(f32::from_bits(data[0]) as i32 as u32)
                        }
                    };
                    for i in 0..slots {
                        output[offset + i] = v;
                    }
                }
                crate::makepad_script::pod::ScriptPodTyTag::F16 => {
                    let v = pod_heap::f16_to_f32(data[0] as u16);
                    let v = match attr_format.f32_write_kind() {
                        DrawShaderF32WriteKind::Float => v,
                        DrawShaderF32WriteKind::UInt => f32::from_bits(v as u32),
                        DrawShaderF32WriteKind::SInt => f32::from_bits(v as i32 as u32),
                    };
                    for i in 0..slots {
                        output[offset + i] = v;
                    }
                }
                crate::makepad_script::pod::ScriptPodTyTag::U32 | crate::makepad_script::pod::ScriptPodTyTag::AtomicU32 => {
                    let v = match attr_format.f32_write_kind() {
                        DrawShaderF32WriteKind::Float => data[0] as f32,
                        DrawShaderF32WriteKind::UInt | DrawShaderF32WriteKind::SInt => {
                            f32::from_bits(data[0])
                        }
                    };
                    for i in 0..slots {
                        output[offset + i] = v;
                    }
                }
                crate::makepad_script::pod::ScriptPodTyTag::I32 | crate::makepad_script::pod::ScriptPodTyTag::AtomicI32 => {
                    let v = match attr_format.f32_write_kind() {
                        DrawShaderF32WriteKind::Float => data[0] as i32 as f32,
                        DrawShaderF32WriteKind::UInt | DrawShaderF32WriteKind::SInt => {
                            f32::from_bits(data[0])
                        }
                    };
                    for i in 0..slots {
                        output[offset + i] = v;
                    }
                }
                crate::makepad_script::pod::ScriptPodTyTag::Bool => {
                    let v = match attr_format.f32_write_kind() {
                        DrawShaderF32WriteKind::Float => {
                            if data[0] != 0 {
                                1.0
                            } else {
                                0.0
                            }
                        }
                        DrawShaderF32WriteKind::UInt | DrawShaderF32WriteKind::SInt => {
                            f32::from_bits(if data[0] != 0 { 1 } else { 0 })
                        }
                    };
                    for i in 0..slots {
                        output[offset + i] = v;
                    }
                }
                crate::makepad_script::pod::ScriptPodTyTag::Vec => {
                    let vec_ty = pod_type.ty().as_vec().unwrap();
                    let dims = vec_ty.dims();
                    match vec_ty {
                        ScriptPodVec::Vec2f | ScriptPodVec::Vec3f | ScriptPodVec::Vec4f => {
                            for i in 0..dims.min(slots) {
                                output[offset + i] = f32::from_bits(data[i]);
                            }
                        }
                        ScriptPodVec::Vec2h | ScriptPodVec::Vec3h | ScriptPodVec::Vec4h => {
                            for i in 0..dims.min(slots) {
                                if i & 1 == 1 {
                                    output[offset + i] =
                                        pod_heap::f16_to_f32((data[i >> 1] >> 16) as u16);
                                } else {
                                    output[offset + i] = pod_heap::f16_to_f32(data[i >> 1] as u16);
                                }
                            }
                        }
                        ScriptPodVec::Vec2u | ScriptPodVec::Vec3u | ScriptPodVec::Vec4u => {
                            for i in 0..dims.min(slots) {
                                output[offset + i] = match attr_format.f32_write_kind() {
                                    DrawShaderF32WriteKind::Float => data[i] as f32,
                                    DrawShaderF32WriteKind::UInt | DrawShaderF32WriteKind::SInt => {
                                        f32::from_bits(data[i])
                                    }
                                };
                            }
                        }
                        ScriptPodVec::Vec2i | ScriptPodVec::Vec3i | ScriptPodVec::Vec4i => {
                            for i in 0..dims.min(slots) {
                                output[offset + i] = match attr_format.f32_write_kind() {
                                    DrawShaderF32WriteKind::Float => data[i] as i32 as f32,
                                    DrawShaderF32WriteKind::UInt | DrawShaderF32WriteKind::SInt => {
                                        f32::from_bits(data[i])
                                    }
                                };
                            }
                        }
                        ScriptPodVec::Vec2b | ScriptPodVec::Vec3b | ScriptPodVec::Vec4b => {
                            for i in 0..dims.min(slots) {
                                output[offset + i] = match attr_format.f32_write_kind() {
                                    DrawShaderF32WriteKind::Float => {
                                        if data[i] != 0 {
                                            1.0
                                        } else {
                                            0.0
                                        }
                                    }
                                    DrawShaderF32WriteKind::UInt | DrawShaderF32WriteKind::SInt => {
                                        f32::from_bits(if data[i] != 0 { 1 } else { 0 })
                                    }
                                };
                            }
                        }
                    }
                }
                crate::makepad_script::pod::ScriptPodTyTag::Mat => {
                    let mat_ty = pod_type.ty().as_mat().unwrap();
                    // Matrices are stored as f32 arrays (column-major order)
                    let dim = mat_ty.dim();
                    for i in 0..dim.min(slots) {
                        output[offset + i] = f32::from_bits(data[i]);
                    }
                }
                _ => {
                    // For other pod types, default to 0
                    for i in 0..slots {
                        output[offset + i] = 0.0;
                    }
                }
            }
            return;
        }

        // Default: fill with zeros
        for i in 0..slots {
            output[offset + i] = 0.0;
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn compile_shader(&mut self, vm: &mut ScriptVm, _apply: &Apply, value: ScriptValue) {
        if let Some(io_self) = value.as_object() {
            // The object cache is keyed by HEAP as well as object: a splash
            // isolate has its own heap, and an object index there says
            // nothing about the same index in the app heap.
            let heap_key = vm.heap_key();
            {
                let cx = vm.host.cx();
                if let Some(&shader_id) = cx.draw_shaders.cache_object_id_to_shader.get(&(heap_key, io_self)) {
                    self.finalize_cached_shader(vm, shader_id);
                    return;
                }
            }

            let fnhash = crate::makepad_script::shader_compile::shader_functions_hash(vm.heap(), vm.bx.code_ref(), io_self);
            let pipe = DrawVars::pipeline_state_hash(vm.heap(), io_self);
            {
                let cx = vm.host.cx();
                if let Some(&shader_id) = cx.draw_shaders.cache_functions_to_shader.get(&fnhash) {
                    let cx = vm.host.cx_mut();
                    cx.draw_shaders
                        .cache_object_id_to_shader
                        .insert((heap_key, io_self), shader_id);
                    self.finalize_cached_shader(vm, shader_id);
                    return;
                }
            }

            let const_table = vm.host.cx().shader_const_table_mode();
            // A shader pack entry binds the shader without a compile.
            if !vm.host.cx().draw_shaders.packs.is_empty() || cfg!(makepad_precompiled_shaders) {
                let key = crate::makepad_script::shader_pack::shader_pack_key(vm, io_self, const_table);
                let lookup = vm.host.cx().draw_shaders.packs.lookup(key);
                if let Some(entry) = lookup.entry() {
                    let desc = entry.to_desc(vm.heap(), io_self);
                    let code = CxDrawShaderCode::Separate { vertex: entry.vertex().to_owned(), fragment: entry.fragment().to_owned() };
                    self.register_compiled_shader(vm, io_self, code, &desc, heap_key, fnhash, pipe);
                    return;
                }
                Self::log_shader_pack_miss(vm, io_self, key);
                #[cfg(makepad_precompiled_shaders)]
                return;
            }

            #[cfg(not(makepad_precompiled_shaders))]
            {
                // A web build recording a shader pack (a trace host's pass 1): every shader that
                // compiles here, the shaders the run really compiles, whatever made them.
                if vm.host.cx().draw_shaders.packs.is_recording() {
                    DrawVars::record_shader_pack_entry(vm, io_self);
                }
                let Some((output, code)) = Self::compile_glsl(vm, io_self, const_table) else {
                    return;
                };
                let desc = DrawShaderDesc::from_output(vm.heap(), &output);
                self.register_compiled_shader(vm, io_self, code, &desc, heap_key, fnhash, pipe);
            }
        }
    }

    /// A shader the shader pack does not hold: say which, and where it is
    /// written (the collect run never applied it: a widget that creates it
    /// from data needs a `scan:` entry). Without a compiler it is not
    /// drawn; with one it is compiled now. Once per shader object.
    #[cfg(target_arch = "wasm32")]
    fn log_shader_pack_miss(vm: &ScriptVm, io_self: ScriptObject, key: u64) {
        let name = vm.heap()
            .object_type_name_in_chain(io_self)
            .map(|id| format!("{}", id))
            .unwrap_or_else(|| format!("<script object {}>", io_self.index()));
        let at = [id!(fragment), id!(pixel), id!(vertex)]
            .iter()
            .find_map(|m| match vm.object_method(io_self, *m).as_object() {
                Some(f) => match vm.heap().fn_script_ip(f) {
                    Some(ip) => vm.ip_to_loc(ip).map(|loc| loc.to_string()),
                    _ => None,
                },
                None => None,
            })
            .unwrap_or_else(|| "unknown".to_string());
        #[cfg(makepad_precompiled_shaders)]
        {
            crate::shader_error::note(format!("{name}: not in the shader pack"));
            crate::error!(
                "not in pack: draw shader '{}' ({}, key {:016x}); this build has no shader compiler: it will NOT be drawn. Add the widget that creates it to a `scan:` list",
                name,
                at,
                key
            );
        }
        #[cfg(not(makepad_precompiled_shaders))]
        crate::log!(
            "not in pack: draw shader '{}' ({}, key {:016x}), compiled at run time. Add the widget that creates it to a `scan:` list",
            name,
            at,
            key
        );
    }

    /// The GLSL ES 3.0 compile of a draw shader: the web backend's code, as
    /// the web build runs it and a shader pack recording repeats it. `None`
    /// (logged) when the shader does not compile.
    #[cfg(not(all(target_arch = "wasm32", makepad_precompiled_shaders)))]
    pub(crate) fn compile_glsl(
        vm: &mut ScriptVm,
        io_self: ScriptObject,
        const_table: bool,
    ) -> Option<(CompiledShader, CxDrawShaderCode)> {
        let opts = crate::makepad_script::shader_compile::ShaderCompileOptions::new(ShaderBackend::Glsl, const_table, false, true, false);
        let output = crate::makepad_script::shader_compile::compile_shader(vm, io_self, &opts);
        if output.has_errors() {
            Self::log_shader_compile_failure(vm, io_self, &output);
            return None;
        }
        output.warn_inline_size(vm, io_self);
        let view = output.view();
        let (vertex, fragment) = (view.glsl_vertex.to_owned(), view.glsl_fragment.to_owned());
        Some((output, CxDrawShaderCode::Separate { vertex, fragment }))
    }

    /// Bind a compiled (or shader pack) draw shader: reuse the shader of the
    /// same code and pipeline state, or build its mapping and register it.
    #[cfg(target_arch = "wasm32")]
    #[allow(clippy::too_many_arguments)]
    fn register_compiled_shader(
        &mut self,
        vm: &mut ScriptVm,
        io_self: ScriptObject,
        code: CxDrawShaderCode,
        desc: &DrawShaderDesc,
        heap_key: usize,
        fnhash: LiveId,
        pipe: LiveId,
    ) {
        {
            let cx = vm.host.cx();
            if let Some(shader_id) = cx.draw_shaders.code_hit(&code, pipe, desc.table_consts()) {
                let cx = vm.host.cx_mut();
                cx.draw_shaders
                    .cache_object_id_to_shader
                    .insert((heap_key, io_self), shader_id);
                cx.draw_shaders
                    .cache_functions_to_shader
                    .insert(fnhash, shader_id);
                self.finalize_cached_shader(vm, shader_id);
                return;
            }
        }

        let geometry_id = if let Some(vb_obj) = Self::find_vertex_buffer_object(vm.heap(), io_self) {
            let buffer_value =
                vm.value_trap(vb_obj, id!(buffer));
            if let Some(handle) = buffer_value.as_handle() {
                vm.heap()
                    .handle_ref::<crate::geometry::Geometry>(handle)
                    .map(|g: &crate::geometry::Geometry| g.geometry_id())
            } else {
                None
            }
        } else {
            None
        };

        let source = vm.new_object_ref(io_self);
        let mut mapping = CxDrawShaderMapping::from_desc(
            source,
            code.clone(),
            vm,
            desc,
            geometry_id,
        );
        mapping.fill_scope_uniforms_buffer(vm.heap(), &vm.trap());

        if !self.bind_dyn_instances(vm, io_self, &mapping) {
            return;
        }
        self.fit_dyn_uniforms(mapping.dyn_uniforms.total_slots);

        // The shader's type name, for GPU diagnostics (`gpu.shaders`).
        let debug_id = vm.heap().object_type_name_in_chain(io_self).unwrap_or(LiveId(0));
        let cx = vm.host.cx_mut();
        mapping.scope_uniforms_gen = cx.next_uniform_gen();
        let index = cx.draw_shaders.shaders.len();
        cx.draw_shaders.shaders.push(CxDrawShader {
            debug_id,
            pipeline_id: None,
            mapping,
        });

        let shader_id = DrawShaderId { index };
        cx.draw_shaders
            .cache_object_id_to_shader
            .insert((heap_key, io_self), shader_id);
        cx.draw_shaders
            .cache_functions_to_shader
            .insert(fnhash, shader_id);
        cx.draw_shaders.cache_code_to_shader.insert((code, pipe), shader_id);
        cx.draw_shaders.compile_set.insert(index);

        self.draw_shader_id = Some(shader_id);
        self.geometry_id = geometry_id;
    }

    /// The vertex-buffer marker object on the shader object's prototype
    /// chain, if it has one (a heap walk, no compile).
    pub fn find_vertex_buffer_object(heap: &ScriptHeap, io_self: ScriptObject) -> Option<ScriptObject> {
        let mut current = Some(io_self);
        while let Some(obj) = current {
            if let Some(found) = heap.map_entries(obj).into_iter().find_map(|e| {
                let value = e.value;
                let value_obj = value.as_object()?;
                (heap.as_shader_io(value_obj) == Some(SHADER_IO_VERTEX_BUFFER)).then_some(value_obj)
            }) {
                return Some(found);
            }
            current = heap.proto(obj).as_object();
        }
        None
    }

    /// The shader's pipeline state as a hash: its `color_format` and
    /// `blend_op` declarations, which the backends bake into its pipelines,
    /// so two shaders of one code but another state never share an entry.
    pub fn pipeline_state_hash(heap: &ScriptHeap, obj: ScriptObject) -> LiveId {
        let mut hash = LiveId(LiveId::SEED);
        for key in [id!(color_format), id!(blend_op)] {
            hash = hash.id_append(key);
            if let Some(v) = heap.value(obj, key.into(), NoTrap).as_id() {
                hash = hash.id_append(v);
            }
        }
        hash
    }

    /// Point this draw's dyn instance window at a newly compiled shader's
    /// fields (the script-declared `name: instance(..)` values, set by name
    /// with [`Self::set_dyn_instance`]). A shader declaring more of them
    /// than a draw holds ([`DRAW_CALL_DYN_INSTANCES`] floats) fails like a
    /// compile error: reported, never registered, not drawn.
    pub fn bind_dyn_instances(&mut self, vm: &ScriptVm, io_self: ScriptObject, mapping: &CxDrawShaderMapping) -> bool {
        let total = mapping.dyn_instances.total_slots;
        if total > self.dyn_instances.len() {
            let name = vm.heap()
                .object_type_name_in_chain(io_self)
                .map(|id| format!("{}", id))
                .unwrap_or_else(|| format!("<script object {}>", io_self.index()));
            let names: Vec<String> = mapping.dyn_instances.inputs.iter().map(|i| format!("{} ({})", i.id, i.slots)).collect();
            let report = format!(
                "{name}: its instance fields take {total} floats, more than the {} a draw holds: {}",
                self.dyn_instances.len(),
                names.join(", ")
            );
            crate::shader_error::note(report.clone());
            crate::error!("draw shader {report}; it will NOT be drawn");
            return false;
        }
        self.dyn_instance_start = self.dyn_instances.len() - total;
        self.dyn_instance_slots = mapping.instances.total_slots;
        true
    }

    /// The script-declared instance fields of this draw's shader (name,
    /// offset, floats): what [`Self::set_dyn_instance`] can set.
    pub fn dyn_instance_inputs<'a>(&self, cx: &'a Cx) -> &'a [DrawShaderInput] {
        match self.draw_shader_id {
            Some(id) => &cx.draw_shaders.shaders[id.index].mapping.dyn_instances.inputs,
            None => &[],
        }
    }

    /// Helper to finalize shader setup after finding a cached shader ID.
    /// Uses the geometry_id stored on the mapping instead of re-running pre_collect_shader_io.
    pub fn finalize_cached_shader(&mut self, vm: &mut ScriptVm, shader_id: DrawShaderId) {
        let cx = vm.host.cx();
        let mapping = &cx.draw_shaders.shaders[shader_id.index].mapping;

        // A registered shader's dyn instances fit (`bind_dyn_instances`).
        self.dyn_instance_start = self.dyn_instances.len() - mapping.dyn_instances.total_slots;
        self.dyn_instance_slots = mapping.instances.total_slots;
        let floats = mapping.dyn_uniforms.total_slots;
        self.fit_dyn_uniforms(floats);

        // Set draw_shader on self
        self.draw_shader_id = Some(shader_id);

        // Use the geometry_id stored on the mapping
        self.geometry_id = mapping.geometry_id;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        area::InstanceArea,
        draw_list::{CxDrawCall, CxDrawKind, DrawList},
    };

    /// A registered shader whose dyn-uniform block is `slots` floats: one
    /// uniform `id` at offset `slots - 4`.
    fn shader_with_uniform(cx: &mut Cx, id: LiveId, slots: usize) -> DrawShaderId {
        let desc = crate::makepad_script::shader_reflection::DrawShaderDesc::default();
        let mut mapping = cx.with_vm(|vm| {
            CxDrawShaderMapping::from_desc(
                ScriptObjectRef::default(),
                CxDrawShaderCode::Combined { code: String::new() },
                vm,
                &desc,
                None,
            )
        });
        mapping.dyn_uniforms.inputs.push(DrawShaderInput {
            id,
            offset: slots - 4,
            slots: 4,
            attr_format: DrawShaderAttrFormat::F32x1,
            byte_offset: (slots - 4) * 4,
            byte_size: 16,
        });
        mapping.dyn_uniforms.total_slots = slots;
        let index = cx.draw_shaders.shaders.len();
        cx.draw_shaders.shaders.push(CxDrawShader {
            debug_id: id,
            pipeline_id: None,
            mapping,
        });
        DrawShaderId { index }
    }

    /// A theme swap re-registers a widget's shader with a larger dyn-uniform
    /// block while the area still points at the draw call of the old shader.
    /// Writes through that area are stale (the next draw re-emits the call);
    /// they must not index the old call's smaller block.
    #[test]
    fn area_writes_skip_a_draw_call_of_another_shader() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let old = shader_with_uniform(&mut cx, live_id!(old_u), 4);
        let new = shader_with_uniform(&mut cx, live_id!(new_u), 8);
        let list = DrawList::new(&mut cx);
        let redraw_id = cx.draw_lists[list.id()].redraw_id;
        let item = cx.draw_lists[list.id()].draw_items.push_item(
            1,
            CxDrawKind::DrawCall(CxDrawCall {
                draw_shader_id: old,
                options: Default::default(),
                append_group_id: 0,
                total_instance_slots: 0,
                draw_call_uniforms: Default::default(),
                geometry_id: None,
                dyn_uniforms: vec![0.0; 4],
                texture_slots: Default::default(),
                uniform_buffer_slots: Default::default(),
                instance_dirty: false,
                uniforms_dirty: false,
                uniforms_gen: 0,
                turtle_depth: 0.0,
                pick_id: 0,
            }),
        );
        item.instances = Some(Default::default());
        let area = Area::Instance(InstanceArea {
            draw_list_id: list.id(),
            draw_item_id: 0,
            instance_offset: 0,
            instance_count: 1,
            redraw_id,
        });
        let mut vars = DrawVars {
            area,
            dyn_instance_start: 0,
            dyn_instance_slots: 0,
            options: Default::default(),
            append_group_id: 0,
            draw_shader_id: Some(new),
            geometry_id: None,
            dyn_uniforms: Vec::new(),
            texture_slots: Default::default(),
            uniform_buffer_slots: Default::default(),
            dyn_instances_pad: Default::default(),
            dyn_instances: [0.0; DRAW_CALL_DYN_INSTANCES],
        };
        vars.set_uniform_on_area(&mut cx, live_id!(new_u), &[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(&vars.dyn_uniforms[4..8], &[1.0, 2.0, 3.0, 4.0]);
        let call = cx.draw_lists[list.id()].draw_items[0].draw_call().unwrap();
        assert_eq!(call.dyn_uniforms, vec![0.0; 4], "the old call is left for the next draw");
        assert!(!call.uniforms_dirty);
    }
}
