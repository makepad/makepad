pub use {
    std::{
        any::TypeId,
    },
    crate::{
        makepad_math::*,
        cx::Cx,
        event::Event,
        live_traits::*,
        draw_vars::DrawVars,
        animator::Animator
    }
};

pub fn live_register(cx:&mut Cx) {
    crate::draw_2d::draw_quad::live_register(cx);
    crate::draw_2d::draw_color::live_register(cx);
    crate::draw_2d::draw_text::live_register(cx);
    crate::shader::geometry_gen::live_register(cx);
    crate::shader::std::live_register(cx);
    crate::font::live_register(cx);
}

impl Cx {
    
    pub fn apply_error_tuple_enum_arg_not_found(&mut self, origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], enum_id: LiveId, base: LiveId, arg: usize) {
        self.apply_error(origin, index, nodes, format!("tuple enum too many args for {}::{} arg no {}", enum_id, base, arg))
    }
    
    pub fn apply_error_named_enum_invalid_prop(&mut self, origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], enum_id: LiveId, base: LiveId, prop: LiveId) {
        self.apply_error(origin, index, nodes, format!("named enum invalid property for {}::{} prop: {}", enum_id, base, prop))
    }
    
    pub fn apply_error_wrong_enum_base(&mut self, origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], enum_id: LiveId, base: LiveId) {
        self.apply_error(origin, index, nodes, format!("wrong enum base expected: {} got: {}", enum_id, base))
    }
    
    pub fn apply_error_wrong_struct_name(&mut self, origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], struct_id: LiveId, got_id: LiveId) {
        self.apply_error(origin, index, nodes, format!("wrong struct name expected: {} got: {}", struct_id, got_id))
    }
    
    pub fn apply_error_wrong_type_for_struct(&mut self, origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], struct_id: LiveId) {
        self.apply_error(origin, index, nodes, format!("wrong type for struct: {} prop: {} type:{:?}", struct_id, nodes[index].id, nodes[index].value))
    }
    
    pub fn apply_error_wrong_enum_variant(&mut self, origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], enum_id: LiveId, variant: LiveId) {
        self.apply_error(origin, index, nodes, format!("wrong enum variant for enum: {} got variant: {}", enum_id, variant))
    }
    
    pub fn apply_error_expected_enum(&mut self, origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode]) {
        self.apply_error(origin, index, nodes, format!("expected enum value type, but got {} {:?}", nodes[index].id, nodes[index].value))
    }
    
    pub fn apply_error_no_matching_field(&mut self, origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode]) {
        self.apply_error(origin, index, nodes, format!("no matching field: {}", nodes[index].id))
    }
    
    pub fn apply_error_wrong_type_for_value(&mut self, origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode]) {
        self.apply_error(origin, index, nodes, format!("wrong type for value: {}", nodes[index].id))
    }
    
    pub fn apply_error_component_not_found(&mut self, origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], id: LiveId) {
        self.apply_error(origin, index, nodes, format!("component not found: {}", id))
    }
    
    pub fn apply_error_wrong_value_type_for_primitive(&mut self, origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], prim: &str) {
        self.apply_error(origin, index, nodes, format!("wrong value type. Prop: {} primitive: {} value: {:?}", nodes[index].id, prim, nodes[index].value))
    }
    
    pub fn apply_error_wrong_expression_type_for_primitive(&mut self, origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], prim: &str, b: LiveEval) {
        self.apply_error(origin, index, nodes, format!("wrong expression return. Prop: {} primitive: {} value: {:?}", nodes[index].id, prim, b))
    }
    
    pub fn apply_error_animation_missing_state(&mut self, origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], track: LiveId, state_id: LiveId, ids: &[LiveId]) {
        self.apply_error(origin, index, nodes, format!("animation missing state: {} {} {:?}", track, state_id, ids))
    }
    
    pub fn apply_error_wrong_animation_track_used(&mut self, origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], id: LiveId, expect: LiveId, got: LiveId) {
        self.apply_error(origin, index, nodes, format!("encountered value [{}] with track [{}] whilst animating on track [{}]", id, expect, got))
    }
    
    pub fn apply_error_animate_to_unknown_track(&mut self, origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], id: LiveId, state_id: LiveId) {
        self.apply_error(origin, index, nodes, format!("unknown track {} in animate_to state_id {}", id, state_id))
    }
    
    pub fn apply_error_empty_object(&mut self, origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode]) {
        self.apply_error(origin, index, nodes, format!("Newed empty ClassName, forgot to call 'use'"))
    }
    
    pub fn apply_key_frame_cannot_be_interpolated(&mut self, origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], a: &LiveValue, b: &LiveValue) {
        self.apply_error(origin, index, nodes, format!("key frame values cannot be interpolated {:?} {:?}", a, b))
    }
    
    pub fn apply_animate_missing_apply_block(&mut self, origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode]) {
        self.apply_error(origin, index, nodes, format!("animate missing apply:{{}} block"))
    }
    
    pub fn apply_error_cant_find_target(&mut self, origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], id: LiveId) {
        self.apply_error(origin, index, nodes, format!("cant find target: {}", id))
    }
    
    pub fn apply_error_eval(&mut self, err:LiveError) {
        let live_registry = self.live_registry.borrow();
        println!("{}", live_registry.live_error_to_live_file_error(err));
    }
    
    pub fn apply_error(&mut self, origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], message: String) {
        let live_registry = self.live_registry.borrow();
        if let Some(token_id) = &nodes[index].origin.token_id() {
            let err = LiveError {
                origin,
                message,
                span: (*token_id).into()
            };
            println!("Apply error: {} {:?}", live_registry.live_error_to_live_file_error(err), nodes[index].value);
        }
        else {
            println!("Apply without file, at index {} {} origin: {}", index, message, origin);
        }
    }
    
    // ok so now what. now we should run the expansion
    pub fn live_expand(&mut self) {
        // lets expand the f'er
        let mut errs = Vec::new();
        let mut live_registry = self.live_registry.borrow_mut();
        live_registry.expand_all_documents(&mut errs);
        for err in errs {
            println!("Error expanding live file {}", live_registry.live_error_to_live_file_error(err));
        }
    }
    
    pub fn register_live_body(&mut self, live_body: LiveBody) {
        //println!("START");
        let result = self.live_registry.borrow_mut().register_live_file(
            &live_body.file,
            LiveModuleId::from_str(&live_body.module_path).unwrap(),
            live_body.code,
            live_body.live_type_infos,
            TextPos {line: live_body.line as u32, column: live_body.column as u32}
        );
        //println!("END");
        if let Err(err) = result {
            println!("Error parsing live file {}", err);
        }
    }
    
    fn update_buffer_from_live_value(slots: usize, output: &mut [f32], offset: usize, v: &LiveValue) {
        match slots {
            1 => {
                let v = v.as_float().unwrap_or(0.0) as f32;
                output[offset + 0] = v;
            }
            2 => {
                let v = v.as_vec2().unwrap_or(vec2(0.0, 0.0));
                output[offset + 0] = v.x;
                output[offset + 1] = v.y;
            }
            3 => {
                let v = v.as_vec3().unwrap_or(vec3(0.0, 0.0, 0.0));
                output[offset + 0] = v.x;
                output[offset + 1] = v.y;
                output[offset + 2] = v.z;
            }
            4 => {
                let v = v.as_vec4().unwrap_or(vec4(0.0, 0.0, 0.0, 0.0));
                output[offset + 0] = v.x;
                output[offset + 1] = v.y;
                output[offset + 2] = v.z;
                output[offset + 3] = v.w;
            }
            _ => {
            }
        }
    }
    
    fn update_buffer_from_live_token(slots: usize, output: &mut [f32], offset: usize, v: &LiveToken) {
        match slots {
            1 => {
                let v = v.as_float().unwrap_or(0.0) as f32;
                output[offset + 0] = v;
            }
            4 => {
                let v = v.as_vec4().unwrap_or(vec4(0.0, 0.0, 0.0, 0.0));
                output[offset + 0] = v.x;
                output[offset + 1] = v.y;
                output[offset + 2] = v.z;
                output[offset + 3] = v.w;
            }
            _ => {
            }
        }
    }
    
    pub fn update_shader_tables_with_live_edit(&mut self, mutated_tokens: &[LiveTokenId], live_ptrs: &[LivePtr]) -> bool {
        // OK now.. we have to access our token tables
        let mut change = false;
        let live_registry_rc = self.live_registry.clone();
        let live_registry = live_registry_rc.borrow();
        //println!("{:?}", live_ptrs);
        
        for shader in &mut self.draw_shaders.shaders {
            let mapping = &mut shader.mapping;
            for live_ptr in live_ptrs {
                if let Some(input) = mapping.live_uniforms.inputs.iter().find( | input | input.live_ptr == Some(*live_ptr)) {
                    let node = live_registry.ptr_to_node(*live_ptr);
                    Self::update_buffer_from_live_value(input.slots, &mut mapping.live_uniforms_buf, input.offset, &node.value);
                    change = true;
                }
            }
            for token_id in mutated_tokens {
                if let Some(table_item) = mapping.const_table.table_index.get(token_id) {
                    let doc = live_registry.token_id_to_origin_doc(*token_id);
                    let token = &doc.tokens[token_id.token_index()];
                    
                    Self::update_buffer_from_live_token(table_item.slots, &mut mapping.const_table.table, table_item.offset, token);
                    change = true;
                }
            }
            
        }
        change
    }
}

