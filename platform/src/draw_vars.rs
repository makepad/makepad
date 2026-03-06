use crate::{
    area::Area,
    cx::Cx,
    draw_shader::*,
    geometry::GeometryId,
    makepad_math::*,
    makepad_script::mod_shader::ShaderIoType,
    makepad_script::mod_shader::SHADER_IO_DYN_INSTANCE,
    makepad_script::mod_shader::SHADER_IO_DYN_UNIFORM,
    makepad_script::pod::{ScriptPodTy, ScriptPodVec},
    makepad_script::pod_heap,
    makepad_script::ScriptFnPtr,
    makepad_script::*,
    script::vm::*,
    texture::Texture,
    uniform_buffer::UniformBuffer,
};

#[cfg(target_arch = "wasm32")]
use crate::makepad_script::{
    shader::{ShaderFnCompiler, ShaderMode, ShaderOutput, ShaderType},
    shader_backend::ShaderBackend,
};

pub const DRAW_CALL_DYN_UNIFORMS: usize = 256;
pub const DRAW_CALL_TEXTURE_SLOTS: usize = 8;
pub const DRAW_CALL_UNIFORM_BUFFER_SLOTS: usize = 2;
pub const DRAW_CALL_DYN_INSTANCES: usize = 32;

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
    #[rust([0f32; DRAW_CALL_DYN_UNIFORMS])]
    pub dyn_uniforms: [f32; DRAW_CALL_DYN_UNIFORMS],
    #[rust]
    pub texture_slots: [Option<Texture>; DRAW_CALL_TEXTURE_SLOTS],
    #[rust]
    pub uniform_buffer_slots: [Option<UniformBuffer>; DRAW_CALL_UNIFORM_BUFFER_SLOTS],
    #[rust([0f32; DRAW_CALL_DYN_INSTANCES])]
    pub dyn_instances: [f32; DRAW_CALL_DYN_INSTANCES],
}

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
            self.compile_shader(vm, apply, value);
        }

        // Read draw-call level options from the shader object.
        if let Some(io_self) = value.as_object() {
            let group_value = vm
                .bx
                .heap
                .value(io_self, id!(draw_call_group).into(), NoTrap);
            if let Some(id) = group_value.as_id() {
                self.options.draw_call_group = id;
            } else if let Some(v) = group_value.as_f64() {
                self.options.draw_call_group = LiveId(v as u64);
            }

            let depth_write_value = vm.bx.heap.value(io_self, id!(depth_write).into(), NoTrap);
            if let Some(v) = depth_write_value.as_bool() {
                self.options.depth_write = v;
            } else if let Some(v) = depth_write_value.as_f64() {
                self.options.depth_write = v != 0.0;
            }

        }
        // lets fill our values
        if self.draw_shader_id.is_some() {
            if let Some(io_self) = value.as_object() {
                let cx = vm.host.cx_mut();
                // For eval applies, only update values that exist at the top level (shallow)
                // This avoids re-reading all values from the prototype chain
                let shallow = apply.is_eval();
                self.fill_dyn_instances(cx, &vm.bx.heap, io_self, shallow);
                self.fill_dyn_uniforms(cx, &vm.bx.heap, io_self, shallow);
            }
        }
        // Update areas for animated properties only
        if apply.is_animate() || apply.is_eval() {
            if let Some(io_self) = value.as_object() {
                let cx = vm.host.cx_mut();
                self.update_instance_areas_when_in_object(cx, &vm.bx.heap, io_self);
                self.update_uniform_areas_when_in_object(cx, &vm.bx.heap, io_self);
            }
        }
    }
}

impl DrawVars {
    fn prune_stale_object_shader_cache(vm: &mut ScriptVm) {
        let object_reuse_epoch = vm.bx.heap.object_reuse_epoch();
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
        unsafe {
            std::slice::from_raw_parts(
                (&self.dyn_instances[self.dyn_instance_start - 1] as *const _ as *const f32)
                    .offset(1),
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
                let draw_call = draw_item.kind.draw_call_mut().unwrap();
                let repeat = inst.instance_count;
                let stride = sh.mapping.instances.total_slots;
                let instances = &mut draw_item.instances.as_mut().unwrap()[inst.instance_offset..];
                let inst_slice = self.as_slice();
                let obj_map = heap.map_ref(io_self);
                let mut any_updated = false;

                for input in &sh.mapping.instances.inputs {
                    let key: ScriptValue = input.id.into();
                    if obj_map.contains_key(&key) {
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
                    cx.passes[draw_list.draw_pass_id.unwrap()].paint_dirty = true;
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
                let sh = &cx.draw_shaders[draw_shader_id.index];
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                let draw_item = &mut draw_list.draw_items[inst.draw_item_id];
                let draw_call = draw_item.kind.draw_call_mut().unwrap();
                let obj_map = heap.map_ref(io_self);
                let mut any_updated = false;

                for input in &sh.mapping.dyn_uniforms.inputs {
                    let key: ScriptValue = input.id.into();
                    if obj_map.contains_key(&key) {
                        for i in 0..input.slots {
                            draw_call.dyn_uniforms[input.offset + i] =
                                self.dyn_uniforms[input.offset + i]
                        }
                        any_updated = true;
                    }
                }

                if any_updated {
                    draw_call.uniforms_dirty = true;
                    cx.passes[draw_list.draw_pass_id.unwrap()].paint_dirty = true;
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
                let draw_call = draw_item.kind.draw_call_mut().unwrap();

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
                cx.passes[draw_list.draw_pass_id.unwrap()].paint_dirty = true;
            }
        }
    }

    pub fn update_instance_area_value(&mut self, cx: &mut Cx, id: &[LiveId]) {
        if let Some(draw_shader_id) = self.draw_shader_id {
            if let Some(inst) = self.area.valid_instance(cx) {
                let sh = &cx.draw_shaders[draw_shader_id.index];
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                let draw_item = &mut draw_list.draw_items[inst.draw_item_id];
                let draw_call = draw_item.kind.draw_call_mut().unwrap();

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
                cx.passes[draw_list.draw_pass_id.unwrap()].paint_dirty = true;
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
        let Some(input) = sh.mapping.instances.inputs.iter().find(|input| input.id == inst) else {
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

    pub fn set_dyn_instance(&mut self, cx: &Cx, instance: LiveId, value: &[f32]) {
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
                }
            }
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
                        value[i] = self.dyn_uniforms[offset + i];
                    }
                }
            }
        }
    }

    pub fn set_uniform(&mut self, cx: &Cx, uniform: LiveId, value: &[f32]) {
        if let Some(draw_shader_id) = self.draw_shader_id {
            let sh = &cx.draw_shaders[draw_shader_id.index];
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

    /// Sets a uniform value and also updates the draw call on the area if valid.
    /// This is used to update uniforms after drawing has completed.
    pub fn set_uniform_on_area(&mut self, cx: &mut Cx, id: LiveId, value: &[f32]) {
        if let Some(draw_shader_id) = self.draw_shader_id {
            let sh = &cx.draw_shaders[draw_shader_id.index];

            // Find the uniform input
            if let Some(input) = sh.mapping.dyn_uniforms.inputs.iter().find(|i| i.id == id) {
                let slots = input.slots.min(value.len());

                // Update local dyn_uniforms
                for i in 0..slots {
                    self.dyn_uniforms[input.offset + i] = value[i];
                }

                // Update the draw call if we have a valid area
                if let Some(inst) = self.area.valid_instance(cx) {
                    let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                    let draw_item = &mut draw_list.draw_items[inst.draw_item_id];
                    let draw_call = draw_item.kind.draw_call_mut().unwrap();

                    for i in 0..slots {
                        draw_call.dyn_uniforms[input.offset + i] = value[i];
                    }
                    draw_call.uniforms_dirty = true;
                    cx.passes[draw_list.draw_pass_id.unwrap()].paint_dirty = true;
                }
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
                    let draw_call = draw_item.kind.draw_call_mut().unwrap();

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
                    cx.passes[draw_list.draw_pass_id.unwrap()].paint_dirty = true;
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
            heap.map_ref(io_self)
                .get(&id.into())
                .map(|v| v.value)
                .unwrap_or(NIL)
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
            let v = match attr_format {
                DrawShaderAttrFormat::Float => v as f32,
                DrawShaderAttrFormat::UInt => f32::from_bits(v as u32),
                DrawShaderAttrFormat::SInt => f32::from_bits(v as i32 as u32),
            };
            for i in 0..slots {
                output[offset + i] = v;
            }
            return;
        }

        // Try u40 (common integer format in script)
        if let Some(v) = value.as_u40() {
            let v = match attr_format {
                DrawShaderAttrFormat::Float => v as f32,
                DrawShaderAttrFormat::UInt => f32::from_bits(v as u32),
                DrawShaderAttrFormat::SInt => f32::from_bits(v as i32 as u32),
            };
            for i in 0..slots {
                output[offset + i] = v;
            }
            return;
        }

        // Try f32
        if let Some(v) = value.as_f32() {
            let v = match attr_format {
                DrawShaderAttrFormat::Float => v,
                DrawShaderAttrFormat::UInt => f32::from_bits(v as u32),
                DrawShaderAttrFormat::SInt => f32::from_bits(v as i32 as u32),
            };
            for i in 0..slots {
                output[offset + i] = v;
            }
            return;
        }

        // Try f16
        if let Some(v) = value.as_f16() {
            let v = match attr_format {
                DrawShaderAttrFormat::Float => v,
                DrawShaderAttrFormat::UInt => f32::from_bits(v as u32),
                DrawShaderAttrFormat::SInt => f32::from_bits(v as i32 as u32),
            };
            for i in 0..slots {
                output[offset + i] = v;
            }
            return;
        }

        // Try u32/i32
        if let Some(v) = value.as_u32() {
            let v = match attr_format {
                DrawShaderAttrFormat::Float => v as f32,
                DrawShaderAttrFormat::UInt | DrawShaderAttrFormat::SInt => f32::from_bits(v),
            };
            for i in 0..slots {
                output[offset + i] = v;
            }
            return;
        }
        if let Some(v) = value.as_i32() {
            let v = match attr_format {
                DrawShaderAttrFormat::Float => v as f32,
                DrawShaderAttrFormat::UInt | DrawShaderAttrFormat::SInt => f32::from_bits(v as u32),
            };
            for i in 0..slots {
                output[offset + i] = v;
            }
            return;
        }

        // Try bool
        if let Some(v) = value.as_bool() {
            let v = match attr_format {
                DrawShaderAttrFormat::Float => {
                    if v {
                        1.0
                    } else {
                        0.0
                    }
                }
                DrawShaderAttrFormat::UInt | DrawShaderAttrFormat::SInt => {
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

        // Try repr(u32) enum variant objects used by script APIs.
        // These carry the numeric payload in `_repr_u32_enum_value`.
        if let Some(obj) = value.as_object() {
            let enum_value = heap.value(obj, live_id!(_repr_u32_enum_value).into(), NoTrap);
            if let Some(v) = enum_value.as_f64() {
                let v = match attr_format {
                    DrawShaderAttrFormat::Float => v as f32,
                    DrawShaderAttrFormat::UInt => f32::from_bits(v as u32),
                    DrawShaderAttrFormat::SInt => f32::from_bits(v as i32 as u32),
                };
                for i in 0..slots {
                    output[offset + i] = v;
                }
                return;
            }
            if let Some(v) = enum_value.as_u32() {
                let v = match attr_format {
                    DrawShaderAttrFormat::Float => v as f32,
                    DrawShaderAttrFormat::UInt | DrawShaderAttrFormat::SInt => f32::from_bits(v),
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

            match &pod_type.ty {
                ScriptPodTy::F32 => {
                    let v = match attr_format {
                        DrawShaderAttrFormat::Float => f32::from_bits(data[0]),
                        DrawShaderAttrFormat::UInt => {
                            f32::from_bits(f32::from_bits(data[0]) as u32)
                        }
                        DrawShaderAttrFormat::SInt => {
                            f32::from_bits(f32::from_bits(data[0]) as i32 as u32)
                        }
                    };
                    for i in 0..slots {
                        output[offset + i] = v;
                    }
                }
                ScriptPodTy::F16 => {
                    let v = pod_heap::f16_to_f32(data[0] as u16);
                    let v = match attr_format {
                        DrawShaderAttrFormat::Float => v,
                        DrawShaderAttrFormat::UInt => f32::from_bits(v as u32),
                        DrawShaderAttrFormat::SInt => f32::from_bits(v as i32 as u32),
                    };
                    for i in 0..slots {
                        output[offset + i] = v;
                    }
                }
                ScriptPodTy::U32 | ScriptPodTy::AtomicU32 => {
                    let v = match attr_format {
                        DrawShaderAttrFormat::Float => data[0] as f32,
                        DrawShaderAttrFormat::UInt | DrawShaderAttrFormat::SInt => {
                            f32::from_bits(data[0])
                        }
                    };
                    for i in 0..slots {
                        output[offset + i] = v;
                    }
                }
                ScriptPodTy::I32 | ScriptPodTy::AtomicI32 => {
                    let v = match attr_format {
                        DrawShaderAttrFormat::Float => data[0] as i32 as f32,
                        DrawShaderAttrFormat::UInt | DrawShaderAttrFormat::SInt => {
                            f32::from_bits(data[0])
                        }
                    };
                    for i in 0..slots {
                        output[offset + i] = v;
                    }
                }
                ScriptPodTy::Bool => {
                    let v = match attr_format {
                        DrawShaderAttrFormat::Float => {
                            if data[0] != 0 {
                                1.0
                            } else {
                                0.0
                            }
                        }
                        DrawShaderAttrFormat::UInt | DrawShaderAttrFormat::SInt => {
                            f32::from_bits(if data[0] != 0 { 1 } else { 0 })
                        }
                    };
                    for i in 0..slots {
                        output[offset + i] = v;
                    }
                }
                ScriptPodTy::Vec(vec_ty) => {
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
                                output[offset + i] = match attr_format {
                                    DrawShaderAttrFormat::Float => data[i] as f32,
                                    DrawShaderAttrFormat::UInt | DrawShaderAttrFormat::SInt => {
                                        f32::from_bits(data[i])
                                    }
                                };
                            }
                        }
                        ScriptPodVec::Vec2i | ScriptPodVec::Vec3i | ScriptPodVec::Vec4i => {
                            for i in 0..dims.min(slots) {
                                output[offset + i] = match attr_format {
                                    DrawShaderAttrFormat::Float => data[i] as i32 as f32,
                                    DrawShaderAttrFormat::UInt | DrawShaderAttrFormat::SInt => {
                                        f32::from_bits(data[i])
                                    }
                                };
                            }
                        }
                        ScriptPodVec::Vec2b | ScriptPodVec::Vec3b | ScriptPodVec::Vec4b => {
                            for i in 0..dims.min(slots) {
                                output[offset + i] = match attr_format {
                                    DrawShaderAttrFormat::Float => {
                                        if data[i] != 0 {
                                            1.0
                                        } else {
                                            0.0
                                        }
                                    }
                                    DrawShaderAttrFormat::UInt | DrawShaderAttrFormat::SInt => {
                                        f32::from_bits(if data[i] != 0 { 1 } else { 0 })
                                    }
                                };
                            }
                        }
                    }
                }
                ScriptPodTy::Mat(mat_ty) => {
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
            {
                let cx = vm.host.cx();
                if let Some(&shader_id) = cx.draw_shaders.cache_object_id_to_shader.get(&io_self) {
                    self.finalize_cached_shader(vm, shader_id);
                    return;
                }
            }

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
            output.backend = ShaderBackend::Glsl;
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

            if output.has_errors {
                return;
            }

            output.assign_uniform_buffer_indices(&vm.bx.heap, 3);

            let mut shared_defs = String::new();
            output.create_struct_defs(vm, &mut shared_defs);

            let mut vertex = String::new();
            let mut fragment = String::new();
            output.glsl_create_vertex_shader(vm, &shared_defs, &mut vertex);
            output.glsl_create_fragment_shader(vm, &shared_defs, &mut fragment);

            let code = CxDrawShaderCode::Separate { vertex, fragment };

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

            let geometry_id = if let Some(vb_obj) = output.find_vertex_buffer_object(vm, io_self) {
                let buffer_value =
                    vm.bx
                        .heap
                        .value(vb_obj, id!(buffer).into(), vm.thread().trap.pass());
                if let Some(handle) = buffer_value.as_handle() {
                    vm.bx
                        .heap
                        .handle_ref::<crate::geometry::Geometry>(handle)
                        .map(|g: &crate::geometry::Geometry| g.geometry_id())
                } else {
                    None
                }
            } else {
                None
            };

            let source = vm.bx.heap.new_object_ref(io_self);
            let mut mapping = CxDrawShaderMapping::from_shader_output(
                source,
                code.clone(),
                &vm.bx.heap,
                &output,
                geometry_id,
            );
            mapping.fill_scope_uniforms_buffer(&vm.bx.heap, &vm.thread().trap.pass());

            self.dyn_instance_start = self.dyn_instances.len() - mapping.dyn_instances.total_slots;
            self.dyn_instance_slots = mapping.instances.total_slots;

            let cx = vm.host.cx_mut();
            let index = cx.draw_shaders.shaders.len();
            cx.draw_shaders.shaders.push(CxDrawShader {
                debug_id: LiveId(0),
                os_shader_id: None,
                mapping,
            });

            let shader_id = DrawShaderId { index };
            cx.draw_shaders
                .cache_object_id_to_shader
                .insert(io_self, shader_id);
            cx.draw_shaders
                .cache_functions_to_shader
                .insert(fnhash, shader_id);
            cx.draw_shaders.cache_code_to_shader.insert(code, shader_id);
            cx.draw_shaders.compile_set.insert(index);

            self.draw_shader_id = Some(shader_id);
            self.geometry_id = geometry_id;
        }
    }

    /// Compute a hash of all function IDs on an object by iterating through
    /// the prototype chain and hashing each function's ScriptIp.
    pub fn compute_shader_functions_hash(heap: &ScriptHeap, obj: ScriptObject) -> LiveId {
        let mut hash = LiveId(LiveId::SEED);

        // Walk the prototype chain to collect all functions
        let mut current = Some(obj);
        while let Some(cur_obj) = current {
            // Iterate through the object's map entries
            for (key, value) in heap.map_ref(cur_obj).iter() {
                // Check if the value is a function object
                if let Some(fn_obj) = value.value.as_object() {
                    if let Some(fn_ptr) = heap.as_fn(fn_obj) {
                        // Hash the key (method name)
                        if let Some(key_id) = key.as_id() {
                            hash = hash.id_append(key_id);
                        }
                        // Hash the function pointer
                        match fn_ptr {
                            ScriptFnPtr::Script(ip) => {
                                // Hash the ScriptIp as bytes
                                let ip_bytes = ip.to_u40().to_be_bytes();
                                hash = hash.bytes_append(&ip_bytes);
                            }
                            _ => (),
                        }
                    }
                }
            }
            // Move to prototype
            current = heap.proto(cur_obj).as_object();
        }

        hash
    }

    /// Helper to finalize shader setup after finding a cached shader ID.
    /// Uses the geometry_id stored on the mapping instead of re-running pre_collect_shader_io.
    pub fn finalize_cached_shader(&mut self, vm: &mut ScriptVm, shader_id: DrawShaderId) {
        let cx = vm.host.cx();
        let mapping = &cx.draw_shaders.shaders[shader_id.index].mapping;

        // Set dyn_instance_start and dyn_instance_slots based on mapping
        self.dyn_instance_start = self.dyn_instances.len() - mapping.dyn_instances.total_slots;
        self.dyn_instance_slots = mapping.instances.total_slots;

        // Set draw_shader on self
        self.draw_shader_id = Some(shader_id);

        // Use the geometry_id stored on the mapping
        self.geometry_id = mapping.geometry_id;
    }
}
