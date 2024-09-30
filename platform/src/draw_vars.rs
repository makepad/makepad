use {
    crate::{
        makepad_live_compiler::{
            LiveRegistry,
            TokenSpan,
            LivePtr,
            LiveValue,
            LiveTypeInfo,
            LiveFieldKind,
            LiveModuleId,
            LiveType,
            LiveNode,
            LiveNodeSliceApi
        },
        makepad_live_tokenizer::{LiveErrorOrigin, live_error_origin},
        makepad_shader_compiler::*,
        makepad_live_id::*,
        makepad_math::*,
        cx::Cx,
        texture::{Texture},
        geometry::GeometryId,
        area::Area,
        geometry::{GeometryFields},
        live_traits::*,
        draw_shader::*
    },
};

/*
pub enum ShaderCompileResult {
    Nop,
    Ok
}*/


#[cfg(any(target_arch = "wasm32", target_os="android", target_os="linux"))]
pub const fn shader_enum(i: u32) -> u32 {
    match i {
        1 => 0x3f800000,
        2 => 0x40000000,
        3 => 0x40400000,
        4 => 0x40800000,
        5 => 0x40a00000,
        6 => 0x40c00000,
        7 => 0x40e00000,
        8 => 0x41000000,
        9 => 0x41100000,
        10 => 0x41200000,
        11 => 0x41300000,
        12 => 0x41400000,
        13 => 0x41500000,
        14 => 0x41600000,
        15 => 0x41700000,
        16 => 0x41800000,
        17 => 0x41880000,
        18 => 0x41900000,
        19 => 0x41980000,
        20 => 0x41a00000,
        21 => 0x41a80000,
        22 => 0x41b00000,
        23 => 0x41b80000,
        24 => 0x41c00000,
        25 => 0x41c80000,
        26 => 0x41d00000,
        27 => 0x41d80000,
        28 => 0x41e00000,
        29 => 0x41e80000,
        30 => 0x41f00000,
        31 => 0x41f80000,
        _ => panic!()
    }
}

#[cfg(not(any(target_arch = "wasm32", target_os="android", target_os="linux")))]
pub const fn shader_enum(i: u32) -> u32 {
    if i<1 || i > 31 {
        panic!();
    }
    i
}

pub const DRAW_CALL_USER_UNIFORMS: usize = 16;
pub const DRAW_CALL_TEXTURE_SLOTS: usize = 4;
pub const DRAW_CALL_VAR_INSTANCES: usize = 32;

#[derive(Default, Clone)]
#[repr(C)]
pub struct DrawVars {
    pub area: Area,
    pub (crate) var_instance_start: usize,
    pub (crate) var_instance_slots: usize,
    pub (crate) options: CxDrawShaderOptions,
    pub draw_shader: Option<DrawShader>,
    pub (crate) geometry_id: Option<GeometryId>,
    pub user_uniforms: [f32; DRAW_CALL_USER_UNIFORMS],
    pub texture_slots: [Option<Texture>; DRAW_CALL_TEXTURE_SLOTS],
    pub var_instances: [f32; DRAW_CALL_VAR_INSTANCES]
}

impl LiveHookDeref for DrawVars{}

impl LiveNew for DrawVars {
    fn new(_cx: &mut Cx) -> Self {
        Self::default()
    }
    
    fn live_type_info(_cx: &mut Cx) -> LiveTypeInfo {
        LiveTypeInfo {
            module_id: LiveModuleId::from_str(&module_path!()).unwrap(),
            live_type: std::any::TypeId::of::<Self>(),
            live_ignore: true,
            fields: Vec::new(),
            type_name: id_lut!(DrawVars)
        }
    }
}

impl LiveApply for DrawVars {
    fn apply(&mut self, _cx: &mut Cx, _applyl: &mut Apply, _index: usize, _nodes: &[LiveNode]) -> usize {
        panic!()
    }
}

impl LiveApplyReset for DrawVars {
    fn apply_reset(&mut self, _cx: &mut Cx, _applyl: &mut Apply, _index: usize, _nodes: &[LiveNode]) {
        // do nothing
    }
}

impl LiveHook for DrawVars {}

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
        self.draw_shader.is_some()
    }
    
    pub fn as_slice<'a>(&'a self) -> &'a [f32] {
        unsafe {
            std::slice::from_raw_parts((&self.var_instances[self.var_instance_start - 1] as *const _ as *const f32).offset(1), self.var_instance_slots)
        }
    }
    
    pub fn init_shader(&mut self, cx: &mut Cx, apply: &mut Apply, draw_shader_ptr: DrawShaderPtr, geometry_fields: &dyn GeometryFields) {
        self.draw_shader = None;
        
        if cx.draw_shaders.error_set.contains(&draw_shader_ptr) {
            return
        }
        
        if let Some(item) = cx.draw_shaders.ptr_to_item.get(&draw_shader_ptr) {
            self.draw_shader = Some(DrawShader {
                draw_shader_generation: cx.draw_shaders.generation,
                draw_shader_ptr,
                draw_shader_id: item.draw_shader_id
            });
            self.options = item.options.clone();
        }
        else {
            // create a fingerprint from all the dsl nodes only
            let fingerprint = DrawShaderFingerprint::from_ptr(cx, draw_shader_ptr);
            
            // see if we have it already
            if let Some(fp) = cx.draw_shaders.fingerprints.iter().find( | fp | fp.fingerprint == fingerprint) {
                self.options = CxDrawShaderOptions::from_ptr(cx, draw_shader_ptr);
                cx.draw_shaders.ptr_to_item.insert(draw_shader_ptr, CxDrawShaderItem {
                    draw_shader_id: fp.draw_shader_id,
                    options: self.options.clone()
                });
                self.draw_shader = Some(DrawShader {
                    draw_shader_generation: cx.draw_shaders.generation,
                    draw_shader_ptr,
                    draw_shader_id: fp.draw_shader_id
                });
                return;
            }
            
            // see if another variant errored
            if cx.draw_shaders.error_fingerprints.iter().find( | fp | **fp == fingerprint).is_some() {
                return;
            }

            fn live_type_to_shader_ty(live_type: LiveType) -> Option<ShaderTy> {
                if live_type == LiveType::of::<f32>() {Some(ShaderTy::Float)}
                else if live_type == LiveType::of::<Vec2>() {Some(ShaderTy::Vec2)}
                else if live_type == LiveType::of::<Vec3>() {Some(ShaderTy::Vec3)}
                else if live_type == LiveType::of::<Vec4>() {Some(ShaderTy::Vec4)}
                else {None}
            }
            // ok ! we have to compile it
            //let live_factories = &cx.live_factories;
            let live_registry_cp = cx.live_registry.clone();
            let live_registry = live_registry_cp.borrow();
            
            let result = cx.shader_registry.analyse_draw_shader(&live_registry, draw_shader_ptr, | live_registry, shader_registry, span, draw_shader_query, live_type, draw_shader_def | {
                match draw_shader_query {
                    DrawShaderQuery::DrawShader => {
                        fn recur_expand(
                            live_registry: &LiveRegistry,
                            shader_registry: &ShaderRegistry,
                            level: usize,
                            after_draw_vars: &mut bool,
                            live_type: LiveType,
                            draw_shader_def: &mut DrawShaderDef,
                            span: TokenSpan
                        ) {
                            if let Some(lf) = live_registry.live_type_infos.get(&live_type) {
                                
                                let mut slots = 0;
                                for field in &lf.fields {
                                    if let LiveFieldKind::Deref = field.live_field_kind {
                                        if field.live_type_info.live_type != LiveType::of::<DrawVars>() {
                                            recur_expand(live_registry, shader_registry, level + 1, after_draw_vars, field.live_type_info.live_type, draw_shader_def, span);
                                            continue
                                        }
                                        else{
                                            *after_draw_vars = true;
                                            continue
                                        }
                                    }
                                    if *after_draw_vars {
                                        // lets count sizes
                                        //
                                        let live_type = field.live_type_info.live_type;
                                        if shader_registry.enums.get(&live_type).is_some() {
                                            slots += 1;
                                            //draw_shader_def.enums
                                            
                                            draw_shader_def.add_instance(field.id, ShaderTy::Enum(live_type), span, field.live_field_kind);
                                        }
                                        else {
                                            let ty = live_type_to_shader_ty(live_type).expect("Please only put shader-understandable instance fields after draw_vars");
                                            slots += ty.slots();
                                            draw_shader_def.add_instance(field.id, ty, span, field.live_field_kind);
                                        }
                                    }
                                }
                                // insert padding
                                if level >0 && slots % 2 == 1 {
                                    draw_shader_def.add_instance(LiveId(0), ShaderTy::Float, span, LiveFieldKind::Calc);
                                }
                            }
                        }
                        recur_expand(live_registry, shader_registry, 0, &mut false, live_type, draw_shader_def, span);
                    }
                    DrawShaderQuery::Geometry => {
                        if live_type == geometry_fields.live_type_check() {
                            let mut fields = Vec::new();
                            geometry_fields.geometry_fields(&mut fields);
                            for field in fields {
                                draw_shader_def.add_geometry(field.id, field.ty, span);
                            }
                        }
                        else {
                            eprintln!("lf.get_type() != geometry_fields.live_type_check()");
                        }
                    }
                }
            });
            // ok lets print an error
            match result {
                Err(e) => {
                    cx.draw_shaders.error_set.insert(draw_shader_ptr);
                    cx.draw_shaders.error_fingerprints.push(fingerprint);
                    // ok so. lets get the source for this file id
                    let err = live_registry.live_error_to_live_file_error(e);
                    if std::env::args().find(|v| v == "--message-format=json").is_some(){
                        crate::log::log_with_level(
                            &err.file,
                            err.span.start.line,
                            err.span.start.column,
                            err.span.end.line,
                            err.span.end.column,
                            err.message,
                            crate::log::LogLevel::Error
                        );
                    }
                    else{
                        log!("Error {}", err);
                    }
                }
                Ok(()) => {
                    // OK! SO the shader parsed
                    let draw_shader_id = cx.draw_shaders.shaders.len();
                    
                    //let const_table = DrawShaderConstTable::default();
                    let const_table = cx.shader_registry.compute_const_table(draw_shader_ptr);
                    
                    let mut mapping = CxDrawShaderMapping::from_draw_shader_def(
                        cx.shader_registry.draw_shader_defs.get(&draw_shader_ptr).unwrap(),
                        const_table,
                        DRAW_SHADER_INPUT_PACKING
                    );
                    
                    mapping.update_live_and_user_uniforms(cx, apply);
                    
                    let live_registry_rc = cx.live_registry.clone();
                    let live_registry = live_registry_rc.borrow();
                    let class_node = live_registry.ptr_to_node(draw_shader_ptr.0);
                    
                    let shader_type_name = match &class_node.value {
                        LiveValue::Class {live_type, ..} => {
                            // lets get the type name
                            let lti = live_registry.live_type_infos.get(live_type).unwrap();
                            lti.type_name
                        }
                        _ => LiveId(0)
                    };
                    cx.draw_shaders.fingerprints.push(DrawShaderFingerprint {
                        draw_shader_id,
                        fingerprint
                    });
                    cx.draw_shaders.shaders.push(CxDrawShader {
                        class_prop: class_node.id,
                        type_name: shader_type_name,
                        os_shader_id: None,
                        mapping: mapping
                    });
                    // ok so. maybe we should fill the live_uniforms buffer?
                    self.options = CxDrawShaderOptions::from_ptr(cx, draw_shader_ptr);
                    cx.draw_shaders.ptr_to_item.insert(draw_shader_ptr, CxDrawShaderItem {
                        draw_shader_id,
                        options: self.options.clone()
                    });
                    cx.draw_shaders.compile_set.insert(draw_shader_ptr);
                    // now we simply queue it somewhere somehow to compile.
                    self.draw_shader = Some(DrawShader {
                        draw_shader_generation: cx.draw_shaders.generation,
                        draw_shader_id,
                        draw_shader_ptr
                    });
                    
                    // self.geometry_id = geometry_fields.get_geometry_id();
                    //println!("{:?}", self.geometry_id);
                    // also we should allocate it a Shader object
                }
            }
        }
    }
    
    pub fn update_area_with_self(&mut self, cx: &mut Cx, index: usize, nodes: &[LiveNode]) {
        if let Some(draw_shader) = self.draw_shader {
            if let Some(inst) = self.area.valid_instance(cx) {
                if draw_shader.draw_shader_generation != cx.draw_shaders.generation {
                    return;
                }
                let sh = &cx.draw_shaders[draw_shader.draw_shader_id];
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                let draw_item = &mut draw_list.draw_items[inst.draw_item_id];
                let draw_call = draw_item.kind.draw_call_mut().unwrap();
                
                let repeat = inst.instance_count;
                let stride = sh.mapping.instances.total_slots;
                let instances = &mut draw_item.instances.as_mut().unwrap()[inst.instance_offset..];
                let inst_slice = self.as_slice();
                
                let mut node_iter = nodes.first_child(index);
                while let Some(node_index) = node_iter {
                    let id = nodes[node_index].id;
                    
                    // lets iterate the /*
                    for input in &sh.mapping.live_instances.inputs {
                        if input.id == id {
                            for j in 0..repeat {
                                for i in 0..input.slots {
                                    instances[input.offset + i + j * stride] = inst_slice[input.offset + i]
                                }
                            }
                            draw_call.instance_dirty = true;
                        }
                    }
                    for input in &sh.mapping.user_uniforms.inputs {
                        if input.id == id {
                            for i in 0..input.slots {
                                draw_call.user_uniforms[input.offset + i] = self.user_uniforms[input.offset + i]
                            }
                        }
                        draw_call.uniforms_dirty = true;
                    }
                    node_iter = nodes.next_child(node_index);
                }
                // DONE!
                cx.passes[draw_list.pass_id.unwrap()].paint_dirty = true;
            }
        }
    }
    
    pub fn update_rect(&mut self, cx: &mut Cx, rect: Rect) {
        if let Some(draw_shader) = self.draw_shader {
            if let Some(inst) = self.area.valid_instance(cx) {
                if draw_shader.draw_shader_generation != cx.draw_shaders.generation {
                    return;
                }
                let sh = &cx.draw_shaders[draw_shader.draw_shader_id];
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
                cx.passes[draw_list.pass_id.unwrap()].paint_dirty = true;
            }
        }
    }
    
    pub fn update_area_with_value(&mut self, cx: &mut Cx, id: LiveId, v: &[f32], start: usize, count: usize) {
        if let Some(draw_shader) = self.draw_shader {
            if let Some(inst) = self.area.valid_instance(cx) {
                if draw_shader.draw_shader_generation != cx.draw_shaders.generation {
                    return;
                }
                let sh = &cx.draw_shaders[draw_shader.draw_shader_id];
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                let draw_item = &mut draw_list.draw_items[inst.draw_item_id];
                let draw_call = draw_item.kind.draw_call_mut().unwrap();
                
                let repeat = inst.instance_count.min(count);
                let stride = sh.mapping.instances.total_slots;
                let instances = &mut draw_item.instances.as_mut().unwrap()[inst.instance_offset..];
                
                cx.passes[draw_list.pass_id.unwrap()].paint_dirty = true;
                
                // lets iterate the /*
                for input in &sh.mapping.live_instances.inputs {
                    if input.id == id {
                        for j in start..(start + repeat) {
                            for i in 0..input.slots {
                                instances[input.offset + i + j * stride] = v[i]
                            }
                        }
                        draw_call.instance_dirty = true;
                    }
                    return
                }
                for input in &sh.mapping.user_uniforms.inputs {
                    if input.id == id {
                        for i in 0..input.slots {
                            draw_call.user_uniforms[input.offset + i] = v[i]
                        }
                        draw_call.uniforms_dirty = true;
                        return
                    }
                }
            }
        }
    }
    
    pub fn get_instance(&self, cx: &mut Cx, inst: &[LiveId], value: &mut [f32]){
        if let Some(draw_shader) = self.draw_shader {
            let sh = &cx.draw_shaders[draw_shader.draw_shader_id];
            let self_slice = self.as_slice();
            for input in &sh.mapping.instances.inputs {
                let offset = input.offset;
                let slots = input.slots;
                if input.id == inst[0] {
                    for i in 0..value.len().min(slots) {
                        value[i] = self_slice[offset + i]
                    }
                }
            }
        }
    }
    
    pub fn set_var_instance(&mut self, cx:&Cx, instance: &[LiveId], value: &[f32]) {
        if let Some(draw_shader) = self.draw_shader {
            let sh = &cx.draw_shaders[draw_shader.draw_shader_id];
            for input in &sh.mapping.var_instances.inputs {
                let offset = (self.var_instances.len() - sh.mapping.var_instances.total_slots) + input.offset;
                let slots = input.slots;
                if input.id == instance[0] {
                    for i in 0..value.len().min(slots) {
                        self.var_instances[offset + i] = value[i];
                    }
                }
            }
        }
    }
    
    pub fn get_uniform(&self, cx: &mut Cx, uniform: &[LiveId], value: &mut [f32]){
        if let Some(draw_shader) = self.draw_shader {
            let sh = &cx.draw_shaders[draw_shader.draw_shader_id];
            for input in &sh.mapping.user_uniforms.inputs {
                let offset = input.offset;
                let slots = input.slots;
                if input.id == uniform[0] {
                    for i in 0..value.len().min(slots) {
                        value[i] = self.user_uniforms[offset + i];
                    }
                }
            }
        }
    }
    
    pub fn set_uniform(&mut self, cx:&Cx, uniform: &[LiveId], value: &[f32]) {
        if let Some(draw_shader) = self.draw_shader { 
            let sh = &cx.draw_shaders[draw_shader.draw_shader_id];
            for input in &sh.mapping.user_uniforms.inputs {
                let offset = input.offset;
                let slots = input.slots;
                if input.id == uniform[0] {
                    for i in 0..value.len().min(slots) {
                        self.user_uniforms[offset + i] = value[i]
                    }
                }
            }
        }
    }
    
        
    pub fn init_slicer(
        &mut self,
        cx: &mut Cx,
    ) {
        if let Some(draw_shader) = self.draw_shader {
            let sh = &cx.draw_shaders[draw_shader.draw_shader_id];
            self.var_instance_start = self.var_instances.len() - sh.mapping.var_instances.total_slots;
            self.var_instance_slots = sh.mapping.instances.total_slots;
        }
    }
    
    pub fn before_apply_init_shader(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, _nodes: &[LiveNode], geometry_fields: &dyn GeometryFields) {
        
        let draw_shader_ptr = if let Some(file_id) = apply.from.file_id() {
            let generation = cx.live_registry.borrow().file_id_to_file(file_id).generation;
            DrawShaderPtr(LivePtr::from_index(file_id, index, generation))
        }
        else {
            return
        };
        self.init_shader(cx, apply, draw_shader_ptr, geometry_fields)
    }
    
    pub fn apply_slots(cx: &mut Cx, slots: usize, output: &mut [f32], offset: usize, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        match slots {
            1 => {
                let mut v: f32 = 0.0;
                let index = v.apply(cx, apply, index, nodes);
                output[offset + 0] = v;
                return index;
            }
            2 => {
                let mut v: Vec2 = Vec2::default();
                let index = v.apply(cx, apply, index, nodes);
                output[offset + 0] = v.x;
                output[offset + 1] = v.y;
                return index;
            }
            3 => {
                let mut v: Vec3 = Vec3::default();
                let index = v.apply(cx, apply, index, nodes);
                output[offset + 0] = v.x;
                output[offset + 1] = v.y;
                output[offset + 2] = v.z;
                return index;
            }
            4 => {
                let mut v: Vec4 = Vec4::default();
                let index = v.apply(cx, apply, index, nodes);
                output[offset + 0] = v.x;
                output[offset + 1] = v.y;
                output[offset + 2] = v.z;
                output[offset + 3] = v.w;
                return index;
            }
            _ => {
                return nodes.skip_node(index)
            }
        }
    }
    
    pub fn apply_value(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        
        if nodes[index].origin.node_has_prefix() && nodes[index].value.is_id() {
            return nodes.skip_node(index)
        }
        
        if let Some(draw_shader) = self.draw_shader {
            let id = nodes[index].id;
            if draw_shader.draw_shader_generation != cx.draw_shaders.generation {
                return nodes.skip_node(index);
            }
            let sh = &cx.draw_shaders[draw_shader.draw_shader_id];
            for input in &sh.mapping.user_uniforms.inputs {
                let offset = input.offset;
                let slots = input.slots;
                if input.id == id {
                    return Self::apply_slots(cx, slots, &mut self.user_uniforms, offset, apply, index, nodes);
                }
            }
            for input in &sh.mapping.var_instances.inputs {
                
                let offset = (self.var_instances.len() - sh.mapping.var_instances.total_slots) + input.offset;
                let slots = input.slots;
                if input.id == id {
                    return Self::apply_slots(cx, slots, &mut self.var_instances, offset, apply, index, nodes);
                }
            }
        }
        else { // our shader simply didnt compile
            return nodes.skip_node(index);
        }
        
        if nodes[index].origin.node_has_prefix() {
            return nodes.skip_node(index)
        }
        
        let unknown_shader_props = match nodes[index].id {
            live_id!(debug) => false,
            live_id!(debug_id) => false,
            live_id!(draw_call_group) => false,
            _ => true
        };
        
        if unknown_shader_props && nodes[index].value.is_value_type() {
            cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
        }
        nodes.skip_node(index)
    }
    
    pub fn after_apply_update_self(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode], geometry_fields: &dyn GeometryFields) {
        // alright. so.if we are ApplyFrom::
        if apply.from.is_from_doc() {
            self.init_slicer(cx);
        }
        self.geometry_id = geometry_fields.get_geometry_id();
        self.update_area_with_self(cx, index, nodes);
    }
    
}
