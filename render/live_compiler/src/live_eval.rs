pub use {
    std::{
        any::TypeId,
    },
    makepad_math::*,
    makepad_id_macros::*,
    makepad_live_tokenizer::{
        
        LiveId
    },
    crate::{
        live_error::LiveErrorOrigin,
        live_node_vec::*,
        live_registry::LiveRegistry,
        live_node::*
    }
};


#[derive(Debug)]
pub enum LiveEval {
    Float(f64),
    Vec2(Vec2),
    Vec3(Vec3),
    Vec4(Vec4),
    Int(i64),
    Bool(bool),
    Void
}

pub trait LiveEvalError{
    fn apply_error_wrong_value_in_expression(&mut self, origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], ty: &str);
    fn apply_error_binop_undefined_in_expression(&mut self, origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], op: LiveBinOp, a: LiveEval, b: LiveEval);
    fn apply_error_unop_undefined_in_expression(&mut self, origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], op: LiveUnOp, a: LiveEval);
    fn apply_error_expression_call_not_implemented(&mut self, origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], ident: LiveId, args: usize);
    fn apply_error_cant_find_target(&mut self, origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], id: LiveId);
}

pub fn live_eval(live_registry: &LiveRegistry, start: usize, index: &mut usize, nodes: &[LiveNode], err:&mut Option<&mut dyn LiveEvalError>) -> LiveEval {
    match &nodes[*index].value {
        LiveValue::Float(v) => {
            *index += 1;
            return LiveEval::Float(*v);
        }
        LiveValue::Int(v) => {
            *index += 1;
            return LiveEval::Int(*v);
        }
        LiveValue::Vec2(v) => {
            *index += 1;
            return LiveEval::Vec2(*v);
        }
        LiveValue::Vec3(v) => {
            *index += 1;
            return LiveEval::Vec3(*v);
        }
        LiveValue::Vec4(v) => {
            *index += 1;
            return LiveEval::Vec4(*v);
        }
        LiveValue::Color(c) =>{
            *index += 1;
            return LiveEval::Vec4(Vec4::from_u32(*c));
        }
        LiveValue::Bool(v) => {
            *index += 1;
            return LiveEval::Bool(*v);
        }
        LiveValue::Id(id) => { // look it up from start on up
            *index += 1;
            
            fn last_keyframe_value_from_array(index: usize, nodes: &[LiveNode]) -> Option<usize> {
                if let Some(index) = nodes.last_child(index) {
                    if nodes[index].value.is_object() {
                        return nodes.child_by_name(index, id!(value));
                    }
                    else {
                        return Some(index)
                    }
                }
                return None
            }
            
            fn value_to_live_value(live_registry: &LiveRegistry, index:usize, nodes:&[LiveNode], err:&mut Option<&mut dyn LiveEvalError>)->LiveEval{
                return match &nodes[index].value {
                    LiveValue::Float(val) => LiveEval::Float(*val),
                    LiveValue::Int(val) => LiveEval::Int(*val),
                    LiveValue::Bool(val) => LiveEval::Bool(*val),
                    LiveValue::Vec2(val) => LiveEval::Vec2(*val),
                    LiveValue::Vec3(val) => LiveEval::Vec3(*val),
                    LiveValue::Vec4(val) => LiveEval::Vec4(*val),
                    LiveValue::Color(c) => LiveEval::Vec4(Vec4::from_u32(*c)),
                    LiveValue::Expr{..} => { // expr depends on expr
                        live_eval(live_registry, index, &mut (index + 1), nodes, err)
                    }
                    LiveValue::Array => { // got an animation track. select the last value
                        if let Some(index) = last_keyframe_value_from_array(index, nodes) {
                            match &nodes[index].value {
                                LiveValue::Float(val) => LiveEval::Float(*val),
                                LiveValue::Int(val) => LiveEval::Int(*val),
                                LiveValue::Bool(val) => LiveEval::Bool(*val),
                                _ => {
                                    LiveEval::Void
                                }
                            }
                        }
                        else {
                            LiveEval::Void
                        }
                    },
                    _ => {
                        if let Some(err) = err.as_mut(){err.apply_error_wrong_value_in_expression(live_error_origin!(), index, nodes, "Id reference")};
                        LiveEval::Void
                    }
                }
            }
            
            if let Some(index) = nodes.scope_up_by_name(start - 1, *id) {
                // found ok now what. it depends on the type of the thing here
                value_to_live_value(live_registry, index, nodes, err)
            }
            else if let Some(token_id) = nodes[start].origin.token_id() { // lets find it on live registry via origin

                let origin_file_id = token_id.file_id();
                let expand_index = nodes[start].get_expr_expand_index().unwrap();

                if let Some(ptr) = live_registry.find_scope_ptr_via_expand_index(origin_file_id, expand_index as usize, *id){
                    let (nodes, index) = live_registry.ptr_to_nodes_index(ptr);
                    value_to_live_value(live_registry, index, nodes, err)
                }
                else{
                    LiveEval::Void
                }
            }
            else{
                if let Some(err) = err.as_mut(){err.apply_error_cant_find_target(live_error_origin!(), *index, nodes, *id)};
                LiveEval::Void
            }
        },
        LiveValue::ExprUnOp(op) => {
            *index += 1;
            let a = live_eval(live_registry, start, index, nodes, err);
            let ret = match op {
                LiveUnOp::Not => match a {
                    LiveEval::Bool(va) => LiveEval::Bool(!va),
                    _ => LiveEval::Void
                }
                LiveUnOp::Neg => match a {
                    LiveEval::Float(va) => LiveEval::Float(-va),
                    LiveEval::Int(va) => LiveEval::Int(-va),
                    _ => LiveEval::Void
                }
            };
            if let LiveEval::Void = ret {
                if let Some(err) = err.as_mut(){err.apply_error_unop_undefined_in_expression(live_error_origin!(),  *index, nodes, *op, a)};
            }
            ret
        }
        LiveValue::ExprCall {ident, args} => {
            *index += 1;
            match ident{
                id!(blend) if *args == 2=>{
                    let a = live_eval(live_registry, start, index, nodes, err);
                    let b = live_eval(live_registry, start, index, nodes, err);
                    
                    if let LiveEval::Vec4(va) = a{
                        if let LiveEval::Vec4(vb) = b{
                            // ok so how do we blend this eh.
                            return LiveEval::Vec4(vec4(
                                va.x + (vb.x-va.x) * vb.w,
                                va.y + (vb.y-va.y) * vb.w,
                                va.z + (vb.z-va.z) * vb.w,
                                va.w
                            ))
                        }
                    }
                }
                _=>{}
            }
            if let Some(err) = err{err.apply_error_expression_call_not_implemented(live_error_origin!(), *index, nodes, *ident, *args)};
            LiveEval::Void
        }
        LiveValue::ExprBinOp(op) => {
            *index += 1;
            let a = live_eval(live_registry, start, index, nodes, err);
            let b = live_eval(live_registry, start, index, nodes, err);
            let ret = match op {
                LiveBinOp::Or => match a {
                    LiveEval::Bool(va) => match b {
                        LiveEval::Bool(vb) => LiveEval::Bool(va || vb),
                        _ => LiveEval::Void
                    }
                    _ => LiveEval::Void
                }
                LiveBinOp::And => match a {
                    LiveEval::Bool(va) => match b {
                        LiveEval::Bool(vb) => LiveEval::Bool(va && vb),
                        _ => LiveEval::Void
                    }
                    _ => LiveEval::Void
                },
                LiveBinOp::Eq => match a {
                    LiveEval::Bool(va) => match b {
                        LiveEval::Bool(vb) => LiveEval::Bool(va == vb),
                        _ => LiveEval::Void
                    }
                    LiveEval::Int(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Bool(va == vb),
                        LiveEval::Float(vb) => LiveEval::Bool(va as f64 == vb),
                        _ => LiveEval::Void
                    }
                    LiveEval::Float(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Bool(va == vb as f64),
                        LiveEval::Float(vb) => LiveEval::Bool(va == vb),
                        _ => LiveEval::Void
                    }
                    LiveEval::Vec2(va) => match b {
                        LiveEval::Vec2(vb) => LiveEval::Bool(va == vb),
                        _ => LiveEval::Void
                    }
                    LiveEval::Vec3(va) => match b {
                        LiveEval::Vec3(vb) => LiveEval::Bool(va == vb),
                        _ => LiveEval::Void
                    }
                    LiveEval::Vec4(va) => match b {
                        LiveEval::Vec4(vb) => LiveEval::Bool(va == vb),
                        _ => LiveEval::Void
                    }
                    _ => LiveEval::Void
                },
                LiveBinOp::Ne => match a {
                    LiveEval::Bool(va) => match b {
                        LiveEval::Bool(vb) => LiveEval::Bool(va != vb),
                        _ => LiveEval::Void
                    }
                    LiveEval::Int(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Bool(va != vb),
                        LiveEval::Float(vb) => LiveEval::Bool(va as f64 != vb),
                        _ => LiveEval::Void
                    }
                    LiveEval::Float(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Bool(va != vb as f64),
                        LiveEval::Float(vb) => LiveEval::Bool(va != vb),
                        _ => LiveEval::Void
                    }
                    LiveEval::Vec2(va) => match b {
                        LiveEval::Vec2(vb) => LiveEval::Bool(va != vb),
                        _ => LiveEval::Void
                    }
                    LiveEval::Vec3(va) => match b {
                        LiveEval::Vec3(vb) => LiveEval::Bool(va != vb),
                        _ => LiveEval::Void
                    }
                    LiveEval::Vec4(va) => match b {
                        LiveEval::Vec4(vb) => LiveEval::Bool(va != vb),
                        _ => LiveEval::Void
                    }
                    _ => LiveEval::Void
                },
                LiveBinOp::Lt => match a {
                    LiveEval::Int(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Bool(va < vb),
                        LiveEval::Float(vb) => LiveEval::Bool((va as f64) < vb),
                        _ => LiveEval::Void
                    }
                    LiveEval::Float(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Bool(va < vb as f64),
                        LiveEval::Float(vb) => LiveEval::Bool(va < vb),
                        _ => LiveEval::Void
                    }
                    _ => LiveEval::Void
                },
                LiveBinOp::Le => match a {
                    LiveEval::Int(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Bool(va <= vb),
                        LiveEval::Float(vb) => LiveEval::Bool((va as f64) <= vb),
                        _ => LiveEval::Void
                    }
                    LiveEval::Float(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Bool(va <= vb as f64),
                        LiveEval::Float(vb) => LiveEval::Bool(va <= vb),
                        _ => LiveEval::Void
                    }
                    _ => LiveEval::Void
                },
                LiveBinOp::Gt => match a {
                    LiveEval::Int(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Bool(va > vb),
                        LiveEval::Float(vb) => LiveEval::Bool((va as f64) > vb),
                        _ => LiveEval::Void
                    }
                    LiveEval::Float(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Bool(va > vb as f64),
                        LiveEval::Float(vb) => LiveEval::Bool(va > vb),
                        _ => LiveEval::Void
                    }
                    _ => LiveEval::Void
                },
                LiveBinOp::Ge => match a {
                    LiveEval::Int(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Bool(va >= vb),
                        LiveEval::Float(vb) => LiveEval::Bool((va as f64) >= vb),
                        _ => LiveEval::Void
                    }
                    LiveEval::Float(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Bool(va >= vb as f64),
                        LiveEval::Float(vb) => LiveEval::Bool(va >= vb),
                        _ => LiveEval::Void
                    }
                    _ => LiveEval::Void
                },
                LiveBinOp::Add => match a {
                    LiveEval::Int(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Int(va + vb),
                        LiveEval::Float(vb) => LiveEval::Float((va as f64) + vb),
                        LiveEval::Vec2(vb) => LiveEval::Vec2(vb + va as f32),
                        LiveEval::Vec3(vb) => LiveEval::Vec3(vb + va as f32),
                        LiveEval::Vec4(vb) => LiveEval::Vec4(vb + va as f32),
                        _ => LiveEval::Void
                    }
                    LiveEval::Float(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Float(va + vb as f64),
                        LiveEval::Float(vb) => LiveEval::Float(va + vb),
                        LiveEval::Vec2(vb) => LiveEval::Vec2(vb + va as f32),
                        LiveEval::Vec3(vb) => LiveEval::Vec3(vb + va as f32),
                        LiveEval::Vec4(vb) => LiveEval::Vec4(vb + va as f32),
                        _ => LiveEval::Void
                    }
                    LiveEval::Vec2(va) => match b {
                        LiveEval::Vec2(vb) => LiveEval::Vec2(va + vb),
                        LiveEval::Int(vb) => LiveEval::Vec2(va + vb as f32),
                        LiveEval::Float(vb) => LiveEval::Vec2(va + vb as f32),
                        _ => LiveEval::Void
                    }
                    LiveEval::Vec3(va) => match b {
                        LiveEval::Vec3(vb) => LiveEval::Vec3(va + vb),
                        LiveEval::Int(vb) => LiveEval::Vec3(va + vb as f32),
                        LiveEval::Float(vb) => LiveEval::Vec3(va + vb as f32),
                        _ => LiveEval::Void
                    }
                    LiveEval::Vec4(va) => match b {
                        LiveEval::Vec4(vb) => LiveEval::Vec4(va + vb),
                        LiveEval::Int(vb) => LiveEval::Vec4(va + vb as f32),
                        LiveEval::Float(vb) => LiveEval::Vec4(va + vb as f32),
                        _ => LiveEval::Void
                    }
                    _ => LiveEval::Void
                },
                LiveBinOp::Sub => match a {
                    LiveEval::Int(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Int(va - vb),
                        LiveEval::Float(vb) => LiveEval::Float((va as f64) - vb),
                        LiveEval::Vec2(vb) => LiveEval::Vec2(vb - va as f32),
                        LiveEval::Vec3(vb) => LiveEval::Vec3(vb - va as f32),
                        LiveEval::Vec4(vb) => LiveEval::Vec4(vb - va as f32),
                        _ => LiveEval::Void
                    }
                    LiveEval::Float(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Float(va - vb as f64),
                        LiveEval::Float(vb) => LiveEval::Float(va - vb),
                        LiveEval::Vec2(vb) => LiveEval::Vec2(vb - va as f32),
                        LiveEval::Vec3(vb) => LiveEval::Vec3(vb - va as f32),
                        LiveEval::Vec4(vb) => LiveEval::Vec4(vb - va as f32),
                        _ => LiveEval::Void
                    }
                    LiveEval::Vec2(va) => match b {
                        LiveEval::Vec2(vb) => LiveEval::Vec2(va - vb),
                        LiveEval::Int(vb) => LiveEval::Vec2(va - vb as f32),
                        LiveEval::Float(vb) => LiveEval::Vec2(va - vb as f32),
                        _ => LiveEval::Void
                    }
                    LiveEval::Vec3(va) => match b {
                        LiveEval::Vec3(vb) => LiveEval::Vec3(va - vb),
                        LiveEval::Int(vb) => LiveEval::Vec3(va - vb as f32),
                        LiveEval::Float(vb) => LiveEval::Vec3(va - vb as f32),
                        _ => LiveEval::Void
                    }
                    LiveEval::Vec4(va) => match b {
                        LiveEval::Vec4(vb) => LiveEval::Vec4(va - vb),
                        LiveEval::Int(vb) => LiveEval::Vec4(va - vb as f32),
                        LiveEval::Float(vb) => LiveEval::Vec4(va - vb as f32),
                        _ => LiveEval::Void
                    }
                    _ => LiveEval::Void
                },
                LiveBinOp::Mul => match a {
                    LiveEval::Int(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Int(va * vb),
                        LiveEval::Float(vb) => LiveEval::Float((va as f64) * vb),
                        LiveEval::Vec2(vb) => LiveEval::Vec2(vb * va as f32),
                        LiveEval::Vec3(vb) => LiveEval::Vec3(vb * va as f32),
                        LiveEval::Vec4(vb) => LiveEval::Vec4(vb * va as f32),
                        _ => LiveEval::Void
                    }
                    LiveEval::Float(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Float(va * vb as f64),
                        LiveEval::Float(vb) => LiveEval::Float(va * vb),
                        LiveEval::Vec2(vb) => LiveEval::Vec2(vb * va as f32),
                        LiveEval::Vec3(vb) => LiveEval::Vec3(vb * va as f32),
                        LiveEval::Vec4(vb) => LiveEval::Vec4(vb * va as f32),
                        _ => LiveEval::Void
                    }
                    LiveEval::Vec2(va) => match b {
                        LiveEval::Vec2(vb) => LiveEval::Vec2(va * vb),
                        LiveEval::Int(vb) => LiveEval::Vec2(va * vb as f32),
                        LiveEval::Float(vb) => LiveEval::Vec2(va * vb as f32),
                        _ => LiveEval::Void
                    }
                    LiveEval::Vec3(va) => match b {
                        LiveEval::Vec3(vb) => LiveEval::Vec3(va * vb),
                        LiveEval::Int(vb) => LiveEval::Vec3(va * vb as f32),
                        LiveEval::Float(vb) => LiveEval::Vec3(va * vb as f32),
                        _ => LiveEval::Void
                    }
                    LiveEval::Vec4(va) => match b {
                        LiveEval::Vec4(vb) => LiveEval::Vec4(va * vb),
                        LiveEval::Int(vb) => LiveEval::Vec4(va * vb as f32),
                        LiveEval::Float(vb) => LiveEval::Vec4(va * vb as f32),
                        _ => LiveEval::Void
                    } 
                    _ => LiveEval::Void
                },
                LiveBinOp::Div => match a {
                    LiveEval::Int(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Float(va as f64 / vb as f64),
                        LiveEval::Float(vb) => LiveEval::Float((va as f64) / vb),
                        LiveEval::Vec2(vb) => LiveEval::Vec2(vb / va as f32),
                        LiveEval::Vec3(vb) => LiveEval::Vec3(vb / va as f32),
                        LiveEval::Vec4(vb) => LiveEval::Vec4(vb / va as f32),
                        _ => LiveEval::Void
                    }
                    LiveEval::Float(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Float(va / vb as f64),
                        LiveEval::Float(vb) => LiveEval::Float(va / vb),
                        LiveEval::Vec2(vb) => LiveEval::Vec2(vb / va as f32),
                        LiveEval::Vec3(vb) => LiveEval::Vec3(vb / va as f32),
                        LiveEval::Vec4(vb) => LiveEval::Vec4(vb / va as f32),
                        _ => LiveEval::Void
                    }
                    LiveEval::Vec2(va) => match b {
                        LiveEval::Vec2(vb) => LiveEval::Vec2(va / vb),
                        LiveEval::Int(vb) => LiveEval::Vec2(va / vb as f32),
                        LiveEval::Float(vb) => LiveEval::Vec2(va / vb as f32),
                        _ => LiveEval::Void
                    }
                    LiveEval::Vec3(va) => match b {
                        LiveEval::Vec3(vb) => LiveEval::Vec3(va / vb),
                        LiveEval::Int(vb) => LiveEval::Vec3(va / vb as f32),
                        LiveEval::Float(vb) => LiveEval::Vec3(va / vb as f32),
                        _ => LiveEval::Void
                    }
                    LiveEval::Vec4(va) => match b {
                        LiveEval::Vec4(vb) => LiveEval::Vec4(va / vb),
                        LiveEval::Int(vb) => LiveEval::Vec4(va / vb as f32),
                        LiveEval::Float(vb) => LiveEval::Vec4(va / vb as f32),
                        _ => LiveEval::Void
                    }                     _ => LiveEval::Void
                },
            };
            if let LiveEval::Void = ret {
                if let Some(err) = err{err.apply_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b)};
            }
            ret
        }
        _ => {
            if let Some(err) = err{err.apply_error_wrong_value_in_expression(live_error_origin!(), *index, nodes, "f32")};
            *index = nodes.skip_node(*index);
            LiveEval::Void
        }
    }
}
