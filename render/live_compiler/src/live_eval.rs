pub use {
    std::{
        any::TypeId,
    },
    crate::{
        makepad_math::*,
        makepad_id_macros::*,
        makepad_live_tokenizer::{
            LiveId
        },
        live_error::{LiveError, LiveErrorOrigin},
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
    String(String),
}

impl LiveError {
    fn eval_error_wrong_value_in_expression(origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], ty: &str) ->Self{
        Self::eval_error(origin, index, nodes, format!("wrong value in expression of type {} value: {:?}", ty, nodes[index].value))
    }
    
    fn eval_error_binop_undefined_in_expression(origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], op: LiveBinOp, a: LiveEval, b: LiveEval)->Self {
        Self::eval_error(origin, index, nodes, format!("Operation {:?} undefined between {:?} and {:?}", op, a, b))
    }
    
    fn eval_error_unop_undefined_in_expression(origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], op: LiveUnOp, a: LiveEval)->Self {
        Self::eval_error(origin, index, nodes, format!("Operation {:?} undefined for {:?}", op, a))
    }
    
    fn eval_error_expression_call_not_implemented(origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], ident: LiveId, args: usize)->Self {
        Self::eval_error(origin, index, nodes, format!("Expression call not implemented ident:{} with number of args: {}", ident, args))
    }
    
    fn eval_error_cant_find_target(origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], id: LiveId)->Self {
        Self::eval_error(origin, index, nodes, format!("cant find target: {}", id))
    }
    
    fn eval_error(origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], message: String)->Self{
        LiveError {
            origin,
            message,
            span: nodes[index].origin.token_id().unwrap().into()
        }
    }
}

pub fn live_eval(live_registry: &LiveRegistry, start: usize, index: &mut usize, nodes: &[LiveNode]) -> Result<LiveEval,LiveError> {
    Ok(match &nodes[*index].value {
        LiveValue::Str(_) |
        LiveValue::FittedString(_) |
        LiveValue::InlineString(_) |
        LiveValue::DocumentString {..} => {
            LiveEval::String(live_registry.live_node_as_string(&nodes[*index]).unwrap())
        }
        LiveValue::Float(v) => {
            *index += 1;
            LiveEval::Float(*v)
        }
        LiveValue::Int(v) => {
            *index += 1;
            LiveEval::Int(*v)
        }
        LiveValue::Vec2(v) => {
            *index += 1;
            LiveEval::Vec2(*v)
        }
        LiveValue::Vec3(v) => {
            *index += 1;
            LiveEval::Vec3(*v)
        }
        LiveValue::Vec4(v) => {
            *index += 1;
            LiveEval::Vec4(*v)
        }
        LiveValue::Color(c) => {
            *index += 1;
            LiveEval::Vec4(Vec4::from_u32(*c))
        }
        LiveValue::Bool(v) => {
            *index += 1;
            LiveEval::Bool(*v)
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
            
            fn value_to_live_value(live_registry: &LiveRegistry, index: usize, nodes: &[LiveNode]) -> Result<LiveEval, LiveError> {
                return Ok(match &nodes[index].value {
                    LiveValue::Float(val) => LiveEval::Float(*val),
                    LiveValue::Int(val) => LiveEval::Int(*val),
                    LiveValue::Bool(val) => LiveEval::Bool(*val),
                    LiveValue::Vec2(val) => LiveEval::Vec2(*val),
                    LiveValue::Vec3(val) => LiveEval::Vec3(*val),
                    LiveValue::Vec4(val) => LiveEval::Vec4(*val),
                    LiveValue::Color(c) => LiveEval::Vec4(Vec4::from_u32(*c)),
                    LiveValue::Str(_) |
                    LiveValue::FittedString(_) |
                    LiveValue::InlineString(_) |
                    LiveValue::DocumentString {..} => LiveEval::String(live_registry.live_node_as_string(&nodes[index]).unwrap()),
                    LiveValue::Expr {..} => { // expr depends on expr
                        live_eval(live_registry, index, &mut (index + 1), nodes)?
                    }
                    LiveValue::Array => { // got an animation track. select the last value
                        if let Some(index) = last_keyframe_value_from_array(index, nodes) {
                            match &nodes[index].value {
                                LiveValue::Float(val) => LiveEval::Float(*val),
                                LiveValue::Int(val) => LiveEval::Int(*val),
                                LiveValue::Bool(val) => LiveEval::Bool(*val),
                                _ => {
                                    return Err(LiveError::eval_error_wrong_value_in_expression(live_error_origin!(), index, nodes, "Animation array"))
                                }
                            }
                        }
                        else {
                            return Err(LiveError::eval_error_wrong_value_in_expression(live_error_origin!(), index, nodes, "Animation array"))
                        }
                    },
                    _ => {
                        return Err(LiveError::eval_error_wrong_value_in_expression(live_error_origin!(), index, nodes, "Id referenmce"))
                    }
                })
            }
            
            if let Some(index) = nodes.scope_up_by_name(start - 1, *id) {
                // found ok now what. it depends on the type of the thing here
                value_to_live_value(live_registry, index, nodes)?
            }
            else if let Some(token_id) = nodes[start].origin.token_id() { // lets find it on live registry via origin
                
                let origin_file_id = token_id.file_id();
                let expand_index = nodes[start].get_expr_expand_index().unwrap();
                
                if let Some(ptr) = live_registry.find_scope_ptr_via_expand_index(origin_file_id, expand_index as usize, *id) {
                    let (nodes, index) = live_registry.ptr_to_nodes_index(ptr);
                    value_to_live_value(live_registry, index, nodes)?
                }
                else {
                    return Err(LiveError::eval_error_cant_find_target(live_error_origin!(), *index, nodes, *id))
                }
            }
            else {
                return Err(LiveError::eval_error_cant_find_target(live_error_origin!(), *index, nodes, *id))
            }
        },
        LiveValue::ExprUnOp(op) => {
            *index += 1;
            let a = live_eval(live_registry, start, index, nodes)?;
            match op {
                LiveUnOp::Not => match a {
                    LiveEval::Bool(va) => LiveEval::Bool(!va),
                    _ => return Err(LiveError::eval_error_unop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a))
                }
                LiveUnOp::Neg => match a {
                    LiveEval::Float(va) => LiveEval::Float(-va),
                    LiveEval::Int(va) => LiveEval::Int(-va),
                    _ => return Err(LiveError::eval_error_unop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a))
                }
            }
        }
        LiveValue::ExprCall {ident, args} => {
            *index += 1;
            match ident {
                id!(blend) if *args == 2 => {
                    let a = live_eval(live_registry, start, index, nodes)?;
                    let b = live_eval(live_registry, start, index, nodes)?;
                    if let LiveEval::Vec4(va) = a {
                        if let LiveEval::Vec4(vb) = b {
                            // ok so how do we blend this eh.
                            return Ok(LiveEval::Vec4(vec4(
                                va.x + (vb.x - va.x) * vb.w,
                                va.y + (vb.y - va.y) * vb.w,
                                va.z + (vb.z - va.z) * vb.w,
                                va.w
                            )))
                        }
                    }
                }
                _ => {}
            }
            
            return Err(LiveError::eval_error_expression_call_not_implemented(live_error_origin!(), *index, nodes, *ident, *args))
        }
        LiveValue::ExprBinOp(op) => {
            *index += 1;
            let a = live_eval(live_registry, start, index, nodes)?;
            let b = live_eval(live_registry, start, index, nodes)?;
            match op {
                LiveBinOp::Or => match a {
                    LiveEval::Bool(va) => match b {
                        LiveEval::Bool(vb) => LiveEval::Bool(va || vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                }
                LiveBinOp::And => match a {
                    LiveEval::Bool(va) => match b {
                        LiveEval::Bool(vb) => LiveEval::Bool(va && vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                },
                LiveBinOp::Eq => match a {
                    LiveEval::Bool(va) => match b {
                        LiveEval::Bool(vb) => LiveEval::Bool(va == vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Int(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Bool(va == vb),
                        LiveEval::Float(vb) => LiveEval::Bool(va as f64 == vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Float(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Bool(va == vb as f64),
                        LiveEval::Float(vb) => LiveEval::Bool(va == vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Vec2(va) => match b {
                        LiveEval::Vec2(vb) => LiveEval::Bool(va == vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Vec3(va) => match b {
                        LiveEval::Vec3(vb) => LiveEval::Bool(va == vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Vec4(va) => match b {
                        LiveEval::Vec4(vb) => LiveEval::Bool(va == vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                },
                LiveBinOp::Ne => match a {
                    LiveEval::Bool(va) => match b {
                        LiveEval::Bool(vb) => LiveEval::Bool(va != vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Int(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Bool(va != vb),
                        LiveEval::Float(vb) => LiveEval::Bool(va as f64 != vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Float(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Bool(va != vb as f64),
                        LiveEval::Float(vb) => LiveEval::Bool(va != vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Vec2(va) => match b {
                        LiveEval::Vec2(vb) => LiveEval::Bool(va != vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Vec3(va) => match b {
                        LiveEval::Vec3(vb) => LiveEval::Bool(va != vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Vec4(va) => match b {
                        LiveEval::Vec4(vb) => LiveEval::Bool(va != vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                },
                LiveBinOp::Lt => match a {
                    LiveEval::Int(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Bool(va < vb),
                        LiveEval::Float(vb) => LiveEval::Bool((va as f64) < vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Float(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Bool(va < vb as f64),
                        LiveEval::Float(vb) => LiveEval::Bool(va < vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                },
                LiveBinOp::Le => match a {
                    LiveEval::Int(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Bool(va <= vb),
                        LiveEval::Float(vb) => LiveEval::Bool((va as f64) <= vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Float(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Bool(va <= vb as f64),
                        LiveEval::Float(vb) => LiveEval::Bool(va <= vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                },
                LiveBinOp::Gt => match a {
                    LiveEval::Int(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Bool(va > vb),
                        LiveEval::Float(vb) => LiveEval::Bool((va as f64) > vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Float(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Bool(va > vb as f64),
                        LiveEval::Float(vb) => LiveEval::Bool(va > vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                },
                LiveBinOp::Ge => match a {
                    LiveEval::Int(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Bool(va >= vb),
                        LiveEval::Float(vb) => LiveEval::Bool((va as f64) >= vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Float(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Bool(va >= vb as f64),
                        LiveEval::Float(vb) => LiveEval::Bool(va >= vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                },
                LiveBinOp::Add => match a {
                    LiveEval::Int(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Int(va + vb),
                        LiveEval::Float(vb) => LiveEval::Float((va as f64) + vb),
                        LiveEval::Vec2(vb) => LiveEval::Vec2(vb + va as f32),
                        LiveEval::Vec3(vb) => LiveEval::Vec3(vb + va as f32),
                        LiveEval::Vec4(vb) => LiveEval::Vec4(vb + va as f32),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Float(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Float(va + vb as f64),
                        LiveEval::Float(vb) => LiveEval::Float(va + vb),
                        LiveEval::Vec2(vb) => LiveEval::Vec2(vb + va as f32),
                        LiveEval::Vec3(vb) => LiveEval::Vec3(vb + va as f32),
                        LiveEval::Vec4(vb) => LiveEval::Vec4(vb + va as f32),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Vec2(va) => match b {
                        LiveEval::Vec2(vb) => LiveEval::Vec2(va + vb),
                        LiveEval::Int(vb) => LiveEval::Vec2(va + vb as f32),
                        LiveEval::Float(vb) => LiveEval::Vec2(va + vb as f32),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Vec3(va) => match b {
                        LiveEval::Vec3(vb) => LiveEval::Vec3(va + vb),
                        LiveEval::Int(vb) => LiveEval::Vec3(va + vb as f32),
                        LiveEval::Float(vb) => LiveEval::Vec3(va + vb as f32),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Vec4(va) => match b {
                        LiveEval::Vec4(vb) => LiveEval::Vec4(va + vb),
                        LiveEval::Int(vb) => LiveEval::Vec4(va + vb as f32),
                        LiveEval::Float(vb) => LiveEval::Vec4(va + vb as f32),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                },
                LiveBinOp::Sub => match a {
                    LiveEval::Int(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Int(va - vb),
                        LiveEval::Float(vb) => LiveEval::Float((va as f64) - vb),
                        LiveEval::Vec2(vb) => LiveEval::Vec2(vb - va as f32),
                        LiveEval::Vec3(vb) => LiveEval::Vec3(vb - va as f32),
                        LiveEval::Vec4(vb) => LiveEval::Vec4(vb - va as f32),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Float(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Float(va - vb as f64),
                        LiveEval::Float(vb) => LiveEval::Float(va - vb),
                        LiveEval::Vec2(vb) => LiveEval::Vec2(vb - va as f32),
                        LiveEval::Vec3(vb) => LiveEval::Vec3(vb - va as f32),
                        LiveEval::Vec4(vb) => LiveEval::Vec4(vb - va as f32),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Vec2(va) => match b {
                        LiveEval::Vec2(vb) => LiveEval::Vec2(va - vb),
                        LiveEval::Int(vb) => LiveEval::Vec2(va - vb as f32),
                        LiveEval::Float(vb) => LiveEval::Vec2(va - vb as f32),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Vec3(va) => match b {
                        LiveEval::Vec3(vb) => LiveEval::Vec3(va - vb),
                        LiveEval::Int(vb) => LiveEval::Vec3(va - vb as f32),
                        LiveEval::Float(vb) => LiveEval::Vec3(va - vb as f32),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Vec4(va) => match b {
                        LiveEval::Vec4(vb) => LiveEval::Vec4(va - vb),
                        LiveEval::Int(vb) => LiveEval::Vec4(va - vb as f32),
                        LiveEval::Float(vb) => LiveEval::Vec4(va - vb as f32),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                },
                LiveBinOp::Mul => match a {
                    LiveEval::Int(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Int(va * vb),
                        LiveEval::Float(vb) => LiveEval::Float((va as f64) * vb),
                        LiveEval::Vec2(vb) => LiveEval::Vec2(vb * va as f32),
                        LiveEval::Vec3(vb) => LiveEval::Vec3(vb * va as f32),
                        LiveEval::Vec4(vb) => LiveEval::Vec4(vb * va as f32),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Float(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Float(va * vb as f64),
                        LiveEval::Float(vb) => LiveEval::Float(va * vb),
                        LiveEval::Vec2(vb) => LiveEval::Vec2(vb * va as f32),
                        LiveEval::Vec3(vb) => LiveEval::Vec3(vb * va as f32),
                        LiveEval::Vec4(vb) => LiveEval::Vec4(vb * va as f32),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Vec2(va) => match b {
                        LiveEval::Vec2(vb) => LiveEval::Vec2(va * vb),
                        LiveEval::Int(vb) => LiveEval::Vec2(va * vb as f32),
                        LiveEval::Float(vb) => LiveEval::Vec2(va * vb as f32),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Vec3(va) => match b {
                        LiveEval::Vec3(vb) => LiveEval::Vec3(va * vb),
                        LiveEval::Int(vb) => LiveEval::Vec3(va * vb as f32),
                        LiveEval::Float(vb) => LiveEval::Vec3(va * vb as f32),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Vec4(va) => match b {
                        LiveEval::Vec4(vb) => LiveEval::Vec4(va * vb),
                        LiveEval::Int(vb) => LiveEval::Vec4(va * vb as f32),
                        LiveEval::Float(vb) => LiveEval::Vec4(va * vb as f32),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                },
                LiveBinOp::Div => match a {
                    LiveEval::Int(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Float(va as f64 / vb as f64),
                        LiveEval::Float(vb) => LiveEval::Float((va as f64) / vb),
                        LiveEval::Vec2(vb) => LiveEval::Vec2(vb / va as f32),
                        LiveEval::Vec3(vb) => LiveEval::Vec3(vb / va as f32),
                        LiveEval::Vec4(vb) => LiveEval::Vec4(vb / va as f32),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Float(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Float(va / vb as f64),
                        LiveEval::Float(vb) => LiveEval::Float(va / vb),
                        LiveEval::Vec2(vb) => LiveEval::Vec2(vb / va as f32),
                        LiveEval::Vec3(vb) => LiveEval::Vec3(vb / va as f32),
                        LiveEval::Vec4(vb) => LiveEval::Vec4(vb / va as f32),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Vec2(va) => match b {
                        LiveEval::Vec2(vb) => LiveEval::Vec2(va / vb),
                        LiveEval::Int(vb) => LiveEval::Vec2(va / vb as f32),
                        LiveEval::Float(vb) => LiveEval::Vec2(va / vb as f32),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Vec3(va) => match b {
                        LiveEval::Vec3(vb) => LiveEval::Vec3(va / vb),
                        LiveEval::Int(vb) => LiveEval::Vec3(va / vb as f32),
                        LiveEval::Float(vb) => LiveEval::Vec3(va / vb as f32),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveEval::Vec4(va) => match b {
                        LiveEval::Vec4(vb) => LiveEval::Vec4(va / vb),
                        LiveEval::Int(vb) => LiveEval::Vec4(va / vb as f32),
                        LiveEval::Float(vb) => LiveEval::Vec4(va / vb as f32),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    } _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                },
            }
        }
        _ => {
            return Err(LiveError::eval_error_wrong_value_in_expression(live_error_origin!(), *index, nodes, ""))
        }
    })
}
