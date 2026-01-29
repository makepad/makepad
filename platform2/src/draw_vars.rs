use {
    crate::{
        makepad_script::*,
        makepad_script::mod_shader::SHADER_IO_DYN_INSTANCE,
        makepad_script::mod_shader::SHADER_IO_DYN_UNIFORM,
        makepad_script::mod_shader::ShaderIoType,
        makepad_script::pod_heap,
        makepad_script::pod::{ScriptPodTy, ScriptPodVec},
        makepad_script::ScriptFnPtr,
        makepad_math::*,
        cx::Cx,
        script::vm::*,
        texture::{Texture},
        geometry::GeometryId,
        area::Area,
        draw_shader::*
    },
};

pub const DRAW_CALL_DYN_UNIFORMS: usize = 256;
pub const DRAW_CALL_TEXTURE_SLOTS: usize = 4;
pub const DRAW_CALL_DYN_INSTANCES: usize = 32;

#[derive(Clone, Script, Debug)]
#[repr(C)]
pub struct DrawVars {
    #[rust] pub area: Area,
    #[rust] pub dyn_instance_start: usize,
    #[rust] pub dyn_instance_slots: usize,
    #[rust] pub options: CxDrawShaderOptions,
    #[rust] pub draw_shader_id: Option<DrawShaderId>,
    #[rust] pub geometry_id: Option<GeometryId>,
    #[rust([0f32; DRAW_CALL_DYN_UNIFORMS])] pub dyn_uniforms: [f32; DRAW_CALL_DYN_UNIFORMS],
    #[rust] pub texture_slots: [Option<Texture>; DRAW_CALL_TEXTURE_SLOTS],
    #[rust([0f32; DRAW_CALL_DYN_INSTANCES])] pub dyn_instances: [f32; DRAW_CALL_DYN_INSTANCES]
}

impl ScriptHook for DrawVars{
    fn on_after_apply(&mut self, vm:&mut ScriptVm, apply:&Apply, _scope:&mut Scope, value:ScriptValue){
        if !apply.is_default() && !apply.is_animate(){
            self.compile_shader(vm, apply, value);
        }
        // lets fill our values
        if self.draw_shader_id.is_some() {
            if let Some(io_self) = value.as_object(){
                let cx = vm.host.cx_mut();
                self.fill_dyn_instances(cx, &vm.heap, io_self);
                self.fill_dyn_uniforms(cx, &vm.heap, io_self);
            }
        }
        // Update areas for animated properties only
        if apply.is_animate() {
            if let Some(io_self) = value.as_object() {
                let cx = vm.host.cx_mut();
                self.update_instance_areas_when_in_object(cx, &vm.heap, io_self);
                self.update_uniform_areas_when_in_object(cx, &vm.heap, io_self);
            }
        }
    }
}

impl DrawVars {
    
    pub fn set_texture(&mut self, slot: usize, texture: &Texture) {
        self.texture_slots[slot] = Some(texture.clone());
    }
    
    pub fn empty_texture(&mut self, slot: usize) {
        self.texture_slots[slot] = None;
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
            std::slice::from_raw_parts((&self.dyn_instances[self.dyn_instance_start - 1] as *const _ as *const f32).offset(1), self.dyn_instance_slots)
        }
    }
    
    
    /// Update instance areas only for inputs that exist on the given script object.
    /// Used during animation to only update animated properties.
    fn update_instance_areas_when_in_object(&mut self, cx: &mut Cx, heap: &ScriptHeap, io_self: ScriptObject) {
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
                                instances[input.offset + i + j * stride] = inst_slice[input.offset + i]
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
    fn update_uniform_areas_when_in_object(&mut self, cx: &mut Cx, heap: &ScriptHeap, io_self: ScriptObject) {
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
                            draw_call.dyn_uniforms[input.offset + i] = self.dyn_uniforms[input.offset + i]
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
    
    pub fn update_instance_area_value(&mut self, cx: &mut Cx,  id: &[LiveId]) {
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
                            for k in 0..input.slots{
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
    
    pub fn get_instance(&self, cx: &mut Cx, inst: LiveId, value: &mut [f32]){
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
    
    pub fn set_dyn_instance(&mut self, cx:&Cx, instance: LiveId, value: &[f32]) {
        if let Some(draw_shader_id) = self.draw_shader_id {
            let sh = &cx.draw_shaders[draw_shader_id.index];
            for input in &sh.mapping.dyn_instances.inputs {
                let offset = (self.dyn_instances.len() - sh.mapping.dyn_instances.total_slots) + input.offset;
                let slots = input.slots;
                if input.id == instance {
                    for i in 0..value.len().min(slots) {
                        self.dyn_instances[offset + i] = value[i];
                    }
                }
            }
        }
    }
    
    pub fn get_uniform(&self, cx: &mut Cx, uniform: LiveId, value: &mut [f32]){
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
    
    pub fn set_uniform(&mut self, cx:&Cx, uniform: &[LiveId], value: &[f32]) {
        if let Some(draw_shader_id) = self.draw_shader_id { 
            let sh = &cx.draw_shaders[draw_shader_id.index];
            for input in &sh.mapping.dyn_uniforms.inputs {
                let offset = input.offset;
                let slots = input.slots;
                if input.id == uniform[0] {
                    for i in 0..value.len().min(slots) {
                        self.dyn_uniforms[offset + i] = value[i]
                    }
                }
            }
        }
    }
    
    fn fill_dyn_instances(&mut self, cx: &Cx, heap: &ScriptHeap, io_self: ScriptObject) {
        if let Some(draw_shader_id) = self.draw_shader_id {
            let mapping = &cx.draw_shaders.shaders[draw_shader_id.index].mapping;
            let base_offset = self.dyn_instances.len() - mapping.dyn_instances.total_slots;
            
            for input in &mapping.dyn_instances.inputs {
                let value = Self::extract_shader_io_value(heap, io_self, input.id, SHADER_IO_DYN_INSTANCE);
                if !value.is_nil() && !value.is_err() {
                    Self::write_value_to_f32_slots(heap, value, &mut self.dyn_instances, base_offset + input.offset, input.slots);
                }
            }
        }
    }
    
    fn fill_dyn_uniforms(&mut self, cx: &Cx, heap: &ScriptHeap, io_self: ScriptObject) {
        if let Some(draw_shader_id) = self.draw_shader_id {
            let mapping = &cx.draw_shaders.shaders[draw_shader_id.index].mapping;
            
            for input in &mapping.dyn_uniforms.inputs {
                let value = Self::extract_shader_io_value(heap, io_self, input.id, SHADER_IO_DYN_UNIFORM);
                if !value.is_nil() && !value.is_err() {
                    Self::write_value_to_f32_slots(heap, value, &mut self.dyn_uniforms, input.offset, input.slots);
                }
            }
        }
    }
    
    fn extract_shader_io_value(heap: &ScriptHeap, io_self: ScriptObject, id: LiveId, expected_io_type: ShaderIoType) -> ScriptValue {
        let value = heap.value(io_self, id.into(), NoTrap);
        
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
    pub fn write_value_to_f32_slots(heap: &ScriptHeap, value: ScriptValue, output: &mut [f32], offset: usize, slots: usize) {
        // Try f64 first (most common for abstract numbers)
        if let Some(v) = value.as_f64() {
            let v = v as f32;
            for i in 0..slots {
                output[offset + i] = v;
            }
            return;
        }
        
        // Try u40 (common integer format in script)
        if let Some(v) = value.as_u40() {
            let v = v as f32;
            for i in 0..slots {
                output[offset + i] = v;
            }
            return;
        }
        
        // Try f32
        if let Some(v) = value.as_f32() {
            for i in 0..slots {
                output[offset + i] = v;
            }
            return;
        }
        
        // Try f16
        if let Some(v) = value.as_f16() {
            for i in 0..slots {
                output[offset + i] = v;
            }
            return;
        }
        
        // Try u32/i32
        if let Some(v) = value.as_u32() {
            let v = v as f32;
            for i in 0..slots {
                output[offset + i] = v;
            }
            return;
        }
        if let Some(v) = value.as_i32() {
            let v = v as f32;
            for i in 0..slots {
                output[offset + i] = v;
            }
            return;
        }
        
        // Try bool
        if let Some(v) = value.as_bool() {
            let v = if v { 1.0 } else { 0.0 };
            for i in 0..slots {
                output[offset + i] = v;
            }
            return;
        }
        
        // Try color (u32 RGBA)
        if let Some(c) = value.as_color() {
            let v = Vec4f::from_u32(c);
            if slots >= 1 { output[offset + 0] = v.x; }
            if slots >= 2 { output[offset + 1] = v.y; }
            if slots >= 3 { output[offset + 2] = v.z; }
            if slots >= 4 { output[offset + 3] = v.w; }
            return;
        }
        
        // Try pod (Vec2f, Vec3f, Vec4f, etc.)
        if let Some(pod) = value.as_pod() {
            let (pod_type, data) = heap.pod_data(pod);
            
            match &pod_type.ty {
                ScriptPodTy::F32 => {
                    let v = f32::from_bits(data[0]);
                    for i in 0..slots {
                        output[offset + i] = v;
                    }
                }
                ScriptPodTy::F16 => {
                    let v = pod_heap::f16_to_f32(data[0] as u16);
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
                                    output[offset + i] = pod_heap::f16_to_f32((data[i >> 1] >> 16) as u16);
                                } else {
                                    output[offset + i] = pod_heap::f16_to_f32(data[i >> 1] as u16);
                                }
                            }
                        }
                        ScriptPodVec::Vec2u | ScriptPodVec::Vec3u | ScriptPodVec::Vec4u => {
                            for i in 0..dims.min(slots) {
                                output[offset + i] = data[i] as f32;
                            }
                        }
                        ScriptPodVec::Vec2i | ScriptPodVec::Vec3i | ScriptPodVec::Vec4i => {
                            for i in 0..dims.min(slots) {
                                output[offset + i] = data[i] as i32 as f32;
                            }
                        }
                        ScriptPodVec::Vec2b | ScriptPodVec::Vec3b | ScriptPodVec::Vec4b => {
                            for i in 0..dims.min(slots) {
                                output[offset + i] = if data[i] != 0 { 1.0 } else { 0.0 };
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
                            _=>()
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
