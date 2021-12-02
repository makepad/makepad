pub use {
    std::{
        any::TypeId,
    },
    makepad_live_compiler::*,
    crate::{
        cx::Cx,
        events::Event,
        livetraits::*,
        liveeval::*,
        animation::Animator
    }
};


impl Cx {
    pub fn live_register(&mut self) {
        crate::drawquad::live_register(self);
        crate::drawcolor::live_register(self);
        crate::drawtext::live_register(self);
        crate::geometrygen::live_register(self);
        crate::shader_std::live_register(self);
        crate::font::live_register(self);
    }
    
    pub fn apply_error_tuple_enum_arg_not_found(&mut self, origin:LiveErrorOrigin, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], enum_id: LiveId, base: LiveId, arg: usize) {
        self.apply_error(origin, apply_from, index, nodes, format!("tuple enum too many args for {}::{} arg no {}", enum_id, base, arg))
    }
    
    pub fn apply_error_named_enum_invalid_prop(&mut self, origin:LiveErrorOrigin, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], enum_id: LiveId, base: LiveId, prop: LiveId) {
        self.apply_error(origin, apply_from, index, nodes, format!("named enum invalid property for {}::{} prop: {}", enum_id, base, prop))
    }
    
    pub fn apply_error_wrong_enum_base(&mut self, origin:LiveErrorOrigin, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], enum_id: LiveId, base: LiveId) {
        self.apply_error(origin, apply_from, index, nodes, format!("wrong enum base expected: {} got: {}", enum_id, base))
    }
    
    pub fn apply_error_wrong_struct_name(&mut self, origin:LiveErrorOrigin, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], struct_id: LiveId, got_id: LiveId) {
        self.apply_error(origin, apply_from, index, nodes, format!("wrong struct name expected: {} got: {}", struct_id, got_id))
    }
    
    pub fn apply_error_wrong_type_for_struct(&mut self, origin:LiveErrorOrigin, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], struct_id: LiveId) {
        self.apply_error(origin, apply_from, index, nodes, format!("wrong type for struct: {} prop: {} type:{:?}", struct_id, nodes[index].id, nodes[index].value))
    }
    
    pub fn apply_error_wrong_enum_variant(&mut self, origin:LiveErrorOrigin, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], enum_id: LiveId, variant: LiveId) {
        self.apply_error(origin, apply_from, index, nodes, format!("wrong enum variant for enum: {} got variant: {}", enum_id, variant))
    }
    
    pub fn apply_error_expected_enum(&mut self, origin:LiveErrorOrigin, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        self.apply_error(origin, apply_from, index, nodes, format!("expected enum value type, but got {} {:?}", nodes[index].id, nodes[index].value))
    }
    
    pub fn apply_error_no_matching_field(&mut self, origin:LiveErrorOrigin, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        self.apply_error(origin, apply_from, index, nodes, format!("no matching field: {}", nodes[index].id))
    }
    
    pub fn apply_error_wrong_type_for_value(&mut self, origin:LiveErrorOrigin, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        self.apply_error(origin, apply_from, index, nodes, format!("wrong type for value: {}", nodes[index].id))
    }
    
    pub fn apply_error_component_not_found(&mut self, origin:LiveErrorOrigin, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], id: LiveId) {
        self.apply_error(origin, apply_from, index, nodes, format!("component not found: {}", id))
    }
    
    pub fn apply_error_cant_find_target(&mut self, origin:LiveErrorOrigin, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], id: LiveId) {
        self.apply_error(origin, apply_from, index, nodes, format!("cant find target: {}", id))
    }

    
    pub fn apply_error_wrong_value_type_for_primitive(&mut self, origin:LiveErrorOrigin, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], prim: &str) {
        self.apply_error(origin, apply_from, index, nodes, format!("wrong value type. Prop: {} primitive: {} value: {:?}", nodes[index].id, prim, nodes[index].value))
    }

    pub fn apply_error_wrong_expression_type_for_primitive(&mut self, origin:LiveErrorOrigin, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], prim: &str, b:LiveEval) {
        self.apply_error(origin, apply_from, index, nodes, format!("wrong expression return. Prop: {} primitive: {} value: {:?}", nodes[index].id, prim, b))
    }

    pub fn apply_error_wrong_value_in_expression(&mut self, origin:LiveErrorOrigin, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], ty: &str) {
        self.apply_error(origin, apply_from, index, nodes, format!("wrong value in expression of type {} value: {:?}", ty, nodes[index].value))
    }
    
    pub fn apply_error_binop_undefined_in_expression(&mut self, origin:LiveErrorOrigin, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], op: LiveBinOp, a: LiveEval, b: LiveEval) {
        self.apply_error(origin, apply_from, index, nodes, format!("Operation {:?} undefined between {:?} and {:?}", op, a, b))
    }
    
    pub fn apply_error_unop_undefined_in_expression(&mut self, origin:LiveErrorOrigin, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], op: LiveUnOp, a: LiveEval) {
        self.apply_error(origin, apply_from, index, nodes, format!("Operation {:?} undefined for {:?}", op, a))
    }

    pub fn apply_error_expression_call_not_implemented(&mut self, origin:LiveErrorOrigin, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], ident:LiveId, args:usize) {
        self.apply_error(origin, apply_from, index, nodes, format!("Expression call not implemented ident:{} with number of args: {}", ident, args))
    }

    pub fn apply_error_animation_missing_state(&mut self, origin:LiveErrorOrigin, index: usize, nodes: &[LiveNode], track:LiveId, state_id:LiveId, ids: &[LiveId]) {
        self.apply_error(origin, ApplyFrom::Animate, index, nodes, format!("animation missing state: {} {} {:?}",track, state_id, ids))
    }
    
    pub fn apply_error_wrong_animation_track_used(&mut self, origin:LiveErrorOrigin, index: usize, nodes: &[LiveNode], id: LiveId, expect: LiveId, got: LiveId) {
        self.apply_error(origin, ApplyFrom::Animate, index, nodes, format!("encountered value [{}] with track [{}] whilst animating on track [{}]", id, expect, got))
    }
    
    pub fn apply_error_animate_to_unknown_track(&mut self, origin:LiveErrorOrigin, index: usize, nodes: &[LiveNode], id: LiveId, state_id: LiveId) {
        self.apply_error(origin, ApplyFrom::Animate, index, nodes, format!("unknown track {} in animate_to state_id {}", id, state_id))
    }
    
    pub fn apply_key_frame_cannot_be_interpolated(&mut self, origin:LiveErrorOrigin, index: usize, nodes: &[LiveNode], a: &LiveValue, b: &LiveValue) {
        self.apply_error(origin, ApplyFrom::Animate, index, nodes, format!("key frame values cannot be interpolated {:?} {:?}", a, b))
    }
    
    pub fn apply_error(&mut self, origin:LiveErrorOrigin, _apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], message: String) {
        let live_registry = self.live_registry.borrow();
        if let Some(token_id) = &nodes[index].token_id {
            let err = LiveError {
                origin,
                message,
                span: live_registry.token_id_to_span(*token_id)
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
    /*
    pub fn verify_type_signature(&self, live_ptr: LivePtr, live_type: LiveType) -> bool {
        let live_registry = self.live_registry.borrow();
        let node = live_registry.ptr_to_node(live_ptr);
        if let LiveValue::LiveType(ty) = node.value {
            if ty == live_type {
                return true
            }
        }
        println!("TYPE SIGNATURE VERIFY FAILED");
        false
    }*/
    
    pub fn register_live_body(&mut self, live_body: LiveBody) {
        //println!("START");
        let result = self.live_registry.borrow_mut().parse_live_file(
            &live_body.file,
            LiveModuleId::from_str(&live_body.module_path).unwrap(),
            live_body.code,
            live_body.live_type_infos,
            live_body.line
        );
        //println!("END");
        if let Err(err) = result {
            println!("Error parsing live file {}", err);
        }
    }
    
    pub fn register_factory(&mut self, live_type: LiveType, factory: Box<dyn LiveFactory>) {
        self.live_factories.borrow_mut().insert(live_type, factory);
    }
    
}

