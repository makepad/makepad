pub use {
    std::{
        rc::Rc,
        any::TypeId,
    },
    crate::{
        makepad_math::*,
        makepad_live_id::*,
        makepad_live_tokenizer::{LiveErrorOrigin, live_error_origin},
        live_error::{LiveError},
        live_node_vec::*,
        live_registry::{LiveRegistry,LiveScopeTarget},
        live_node::*
    }
};

/*
#[derive(Debug)]
pub enum LiveEval {
    Float64(f64),
    Vec2(Vec2),
    Vec3(Vec3),
    Vec4(Vec4),
    Int64(i64),
    Bool(bool),
    String(Rc<String>),
}
*/
impl LiveError {
    fn eval_error_wrong_value_in_expression(origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], ty: &str) ->Self{
        Self::eval_error(origin, index, nodes, format!("wrong value in expression of type {} value: {:?}", ty, nodes[index].value))
    }
    
    fn eval_error_binop_undefined_in_expression(origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], op: LiveBinOp, a: LiveValue, b: LiveValue)->Self {
        Self::eval_error(origin, index, nodes, format!("Operation {:?} undefined between {:?} and {:?}", op, a, b))
    }
    
    fn eval_error_unop_undefined_in_expression(origin: LiveErrorOrigin, index: usize, nodes: &[LiveNode], op: LiveUnOp, a: LiveValue)->Self {
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

pub fn live_eval_value(live_registry: &LiveRegistry, index: &mut usize, nodes: &[LiveNode], scope_nodes: &[LiveNode]) -> Result<LiveValue,LiveError> {
    let v = &nodes[*index].value;
    Ok(match v {
        LiveValue::Str(_) |
        LiveValue::InlineString(_) |
        LiveValue::Dependency(_) |
        LiveValue::String(_) |
        LiveValue::Float32(_) |
        LiveValue::Float64(_) |
        LiveValue::Uint64(_) |
        LiveValue::Int64(_) |
        LiveValue::Vec2(_) |
        LiveValue::Vec3(_) |
        LiveValue::Vec4(_) |
        LiveValue::Color(_) |
        LiveValue::Bool(_) =>{
            *index += 1;
            return Ok(v.clone())
        }
        LiveValue::Array => { // got an animation track. select the last value
            fn last_keyframe_value_from_array(index: usize, nodes: &[LiveNode]) -> Option<usize> {
                if let Some(index) = nodes.last_child(index) {
                    if nodes[index].value.is_object() {
                        return nodes.child_by_name(index, live_id!(value).as_field());
                    }
                    else {
                        return Some(index)
                    }
                }
                None
            }
            if let Some(keyframe) = last_keyframe_value_from_array(*index, nodes) {
                *index = nodes.skip_node(*index);
                return Ok(nodes[keyframe].value.clone())
            }                                  
            else {
                return Err(LiveError::eval_error_wrong_value_in_expression(live_error_origin!(), *index, nodes, "Animation array"))
            }
        },
        LiveValue::Expr=>{
            *index += 1;
            return live_eval_value(live_registry, index, nodes, scope_nodes)
        }
        LiveValue::Id(id) => { // look it up from start on up
            *index += 1;
            if let LiveValue::Root(root) = &scope_nodes[0].value {
                // lets find the id
                if let Some(ptr) = root.locals.get(&id){
                    match ptr{
                        LiveScopeTarget::LivePtr(ptr)=>{
                            let doc = live_registry.ptr_to_doc(*ptr);
                            let mut index = ptr.index as usize;
                            return live_eval_value(live_registry, &mut index, &doc.nodes, &doc.nodes)
                        }
                        LiveScopeTarget::LocalPtr(ptr)=>{
                            let mut index = *ptr; 
                            return live_eval_value(live_registry, &mut index, &scope_nodes, &scope_nodes)
                        }
                    }
                }
            }
            return Err(LiveError::eval_error_cant_find_target(live_error_origin!(), *index, nodes, *id))
        },
        LiveValue::ExprUnOp(op) => {
            *index += 1;
            let a = live_eval_value(live_registry, index, nodes, scope_nodes)?;
            match op {
                LiveUnOp::Not => match a {
                    LiveValue::Bool(va) => LiveValue::Bool(!va),
                    _ => return Err(LiveError::eval_error_unop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a))
                }
                LiveUnOp::Neg => match a {
                    LiveValue::Float64(va) => LiveValue::Float64(-va),
                    LiveValue::Int64(va) => LiveValue::Int64(-va),
                    _ => return Err(LiveError::eval_error_unop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a))
                }
            }
        }
        LiveValue::ExprCall {ident, args} => {
            *index += 1;
            match ident {
                live_id!(pow) if *args == 2 => {
                    let a = live_eval_value(live_registry, index, nodes, scope_nodes)?;
                    let b = live_eval_value(live_registry, index, nodes, scope_nodes)?;
                    if let LiveValue::Float64(va) = a {
                        if let LiveValue::Float64(vb) = b {
                            // ok so how do we blend this eh.
                            return Ok(LiveValue::Float64(va.powf(vb)))
                        }
                    }
                }
                live_id!(blend) if *args == 2 => {
                    let a = live_eval_value(live_registry, index, nodes, scope_nodes)?;
                    let b = live_eval_value(live_registry, index, nodes, scope_nodes)?;
                    if let Some(va) = a.as_vec4() {
                        if let Some(vb) = b.as_vec4() {
                            // ok so how do we blend this eh.
                            return Ok(LiveValue::Vec4(vec4(
                                va.x + (vb.x - va.x) * vb.w,
                                va.y + (vb.y - va.y) * vb.w,
                                va.z + (vb.z - va.z) * vb.w,
                                va.w
                            )))
                        }
                    }
                }
                live_id!(mix) if *args == 3 => {
                    let a = live_eval_value(live_registry, index, nodes, scope_nodes)?;
                    let b = live_eval_value(live_registry, index, nodes, scope_nodes)?;
                    let c = live_eval_value(live_registry, index, nodes, scope_nodes)?;
                    
                    if let Some(va) = a.as_vec4() {
                        if let Some(vb) = b.as_vec4() {
                            if let LiveValue::Float64(vc) = c {
                                let vc = vc as f32;
                                // ok so how do we blend this eh.
                                return Ok(LiveValue::Vec4(vec4(
                                    va.x + (vb.x - va.x) * vc,
                                    va.y + (vb.y - va.y) * vc,
                                    va.z + (vb.z - va.z) * vc,
                                    va.w + (vb.w - va.w) * vc
                                )))
                            }
                            if let Some(vc) = c.as_vec4() {
                                // ok so how do we blend this eh.
                                return Ok(LiveValue::Vec4(vec4(
                                    va.x + (vb.x - va.x) * vc.x,
                                    va.y + (vb.y - va.y) * vc.y,
                                    va.z + (vb.z - va.z) * vc.z,
                                    va.w + (vb.w - va.w) * vc.w
                                )))
                            }
                        }
                    }
                }
                live_id!(hsvmod) if *args == 4 => {
                    let orig = live_eval_value(live_registry, index, nodes, scope_nodes)?;
                    let hmod = live_eval_value(live_registry, index, nodes, scope_nodes)?;
                    let smod = live_eval_value(live_registry, index, nodes, scope_nodes)?;
                    let vmod = live_eval_value(live_registry, index, nodes, scope_nodes)?;
                    if let Some(vorig) = orig.as_vec4() {
                        if let LiveValue::Float64(hm) = hmod {
                            if let LiveValue::Float64(sm) = smod {
                                if let LiveValue::Float64(vm) = vmod {
                                    
                                    let mut hsv = vorig.to_hsva();
                                    hsv.x = (hsv.x + (hm as f32)/360.0 + 360.0).rem_euclid(360.);
                                    hsv.z = hsv.z + vm as f32;
                                    hsv.y = hsv.y + sm as f32;
                                                                    
                                    // ok so how do we blend this eh.
                                    return Ok(LiveValue::Vec4(Vec4::from_hsva(hsv)))
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
                        
            return Err(LiveError::eval_error_expression_call_not_implemented(live_error_origin!(), *index, nodes, *ident, *args))
        }
        LiveValue::ExprBinOp(op) => {
            *index += 1;
            let a = live_eval_value(live_registry, index, nodes, scope_nodes)?;
            let b = live_eval_value(live_registry, index, nodes, scope_nodes)?;
            
            match op {
                LiveBinOp::Or => match a {
                    LiveValue::Bool(va) => match b {
                        LiveValue::Bool(vb) => LiveValue::Bool(va || vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                }
                LiveBinOp::And => match a {
                    LiveValue::Bool(va) => match b {
                        LiveValue::Bool(vb) => LiveValue::Bool(va && vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                },
                LiveBinOp::Eq => match a {
                    LiveValue::Bool(va) => match b {
                        LiveValue::Bool(vb) => LiveValue::Bool(va == vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Int64(va) => match b {
                        LiveValue::Int64(vb) => LiveValue::Bool(va == vb),
                        LiveValue::Float64(vb) => LiveValue::Bool(va as f64 == vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Float64(va) => match b {
                        LiveValue::Int64(vb) => LiveValue::Bool(va == vb as f64),
                        LiveValue::Float64(vb) => LiveValue::Bool(va == vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Vec2(va) => match b {
                        LiveValue::Vec2(vb) => LiveValue::Bool(va == vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Vec3(va) => match b {
                        LiveValue::Vec3(vb) => LiveValue::Bool(va == vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Vec4(va) => match b {
                        LiveValue::Vec4(vb) => LiveValue::Bool(va == vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                },
                LiveBinOp::Ne => match a {
                    LiveValue::Bool(va) => match b {
                        LiveValue::Bool(vb) => LiveValue::Bool(va != vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Int64(va) => match b {
                        LiveValue::Int64(vb) => LiveValue::Bool(va != vb),
                        LiveValue::Float64(vb) => LiveValue::Bool(va as f64 != vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Float64(va) => match b {
                        LiveValue::Int64(vb) => LiveValue::Bool(va != vb as f64),
                        LiveValue::Float64(vb) => LiveValue::Bool(va != vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Vec2(va) => match b {
                        LiveValue::Vec2(vb) => LiveValue::Bool(va != vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Vec3(va) => match b {
                        LiveValue::Vec3(vb) => LiveValue::Bool(va != vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Vec4(va) => match b {
                        LiveValue::Vec4(vb) => LiveValue::Bool(va != vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                },
                LiveBinOp::Lt => match a {
                    LiveValue::Int64(va) => match b {
                        LiveValue::Int64(vb) => LiveValue::Bool(va < vb),
                        LiveValue::Float64(vb) => LiveValue::Bool((va as f64) < vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Float64(va) => match b {
                        LiveValue::Int64(vb) => LiveValue::Bool(va < vb as f64),
                        LiveValue::Float64(vb) => LiveValue::Bool(va < vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                },
                LiveBinOp::Le => match a {
                    LiveValue::Int64(va) => match b {
                        LiveValue::Int64(vb) => LiveValue::Bool(va <= vb),
                        LiveValue::Float64(vb) => LiveValue::Bool((va as f64) <= vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Float64(va) => match b {
                        LiveValue::Int64(vb) => LiveValue::Bool(va <= vb as f64),
                        LiveValue::Float64(vb) => LiveValue::Bool(va <= vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                },
                LiveBinOp::Gt => match a {
                    LiveValue::Int64(va) => match b {
                        LiveValue::Int64(vb) => LiveValue::Bool(va > vb),
                        LiveValue::Float64(vb) => LiveValue::Bool((va as f64) > vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Float64(va) => match b {
                        LiveValue::Int64(vb) => LiveValue::Bool(va > vb as f64),
                        LiveValue::Float64(vb) => LiveValue::Bool(va > vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                },
                LiveBinOp::Ge => match a {
                    LiveValue::Int64(va) => match b {
                        LiveValue::Int64(vb) => LiveValue::Bool(va >= vb),
                        LiveValue::Float64(vb) => LiveValue::Bool((va as f64) >= vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Float64(va) => match b {
                        LiveValue::Int64(vb) => LiveValue::Bool(va >= vb as f64),
                        LiveValue::Float64(vb) => LiveValue::Bool(va >= vb),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                },
                LiveBinOp::Add => match a {
                    LiveValue::Int64(va) => match b {
                        LiveValue::Int64(vb) => LiveValue::Int64(va + vb),
                        LiveValue::Float64(vb) => LiveValue::Float64((va as f64) + vb),
                        LiveValue::Vec2(vb) => LiveValue::Vec2(vb + va as f32),
                        LiveValue::Vec3(vb) => LiveValue::Vec3(vb + va as f32),
                        LiveValue::Vec4(vb) => LiveValue::Vec4(vb + va as f32),
                        LiveValue::Color(vb) => LiveValue::Vec4(va as f32 + Vec4::from_u32(vb)),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Float64(va) => match b {
                        LiveValue::Int64(vb) => LiveValue::Float64(va + vb as f64),
                        LiveValue::Float64(vb) => LiveValue::Float64(va + vb),
                        LiveValue::Vec2(vb) => LiveValue::Vec2(va as f32 + vb),
                        LiveValue::Vec3(vb) => LiveValue::Vec3(va as f32 + vb),
                        LiveValue::Vec4(vb) => LiveValue::Vec4(va as f32 + vb),
                        LiveValue::Color(vb) => LiveValue::Vec4(va as f32 + Vec4::from_u32(vb)),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Vec2(va) => match b {
                        LiveValue::Vec2(vb) => LiveValue::Vec2(va + vb),
                        LiveValue::Int64(vb) => LiveValue::Vec2(va + vb as f32),
                        LiveValue::Float64(vb) => LiveValue::Vec2(va + vb as f32),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Vec3(va) => match b {
                        LiveValue::Vec3(vb) => LiveValue::Vec3(va + vb),
                        LiveValue::Int64(vb) => LiveValue::Vec3(va + vb as f32),
                        LiveValue::Float64(vb) => LiveValue::Vec3(va + vb as f32),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Vec4(va) => match b {
                        LiveValue::Vec4(vb) => LiveValue::Vec4(va + vb),
                        LiveValue::Int64(vb) => LiveValue::Vec4(va + vb as f32),
                        LiveValue::Float64(vb) => LiveValue::Vec4(va + vb as f32),
                        LiveValue::Color(vb) => LiveValue::Vec4(va + Vec4::from_u32(vb)),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                },
                LiveBinOp::Sub => match a {
                    LiveValue::Int64(va) => match b {
                        LiveValue::Int64(vb) => LiveValue::Int64(va - vb),
                        LiveValue::Float64(vb) => LiveValue::Float64((va as f64) - vb),
                        LiveValue::Vec2(vb) => LiveValue::Vec2(va as f32 - vb),
                        LiveValue::Vec3(vb) => LiveValue::Vec3(va as f32 - vb),
                        LiveValue::Vec4(vb) => LiveValue::Vec4(va as f32 - vb),
                        LiveValue::Color(vb) => LiveValue::Vec4(va as f32 - Vec4::from_u32(vb)),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Float64(va) => match b {
                        LiveValue::Int64(vb) => LiveValue::Float64(va - vb as f64),
                        LiveValue::Float64(vb) => LiveValue::Float64(va - vb),
                        LiveValue::Vec2(vb) => LiveValue::Vec2(va as f32 - vb),
                        LiveValue::Vec3(vb) => LiveValue::Vec3(va as f32 - vb),
                        LiveValue::Vec4(vb) => LiveValue::Vec4(va as f32 - vb),
                        LiveValue::Color(vb) => LiveValue::Vec4(va as f32 - Vec4::from_u32(vb)),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Vec2(va) => match b {
                        LiveValue::Vec2(vb) => LiveValue::Vec2(va - vb),
                        LiveValue::Int64(vb) => LiveValue::Vec2(va - vb as f32),
                        LiveValue::Float64(vb) => LiveValue::Vec2(va - vb as f32),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Vec3(va) => match b {
                        LiveValue::Vec3(vb) => LiveValue::Vec3(va - vb),
                        LiveValue::Int64(vb) => LiveValue::Vec3(va - vb as f32),
                        LiveValue::Float64(vb) => LiveValue::Vec3(va - vb as f32),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Vec4(va) => match b {
                        LiveValue::Vec4(vb) => LiveValue::Vec4(va - vb),
                        LiveValue::Int64(vb) => LiveValue::Vec4(va - vb as f32),
                        LiveValue::Float64(vb) => LiveValue::Vec4(va - vb as f32),
                        LiveValue::Color(vb) => LiveValue::Vec4(va - Vec4::from_u32(vb)),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                },
                LiveBinOp::Mul => match a {
                    LiveValue::Int64(va) => match b {
                        LiveValue::Int64(vb) => LiveValue::Int64(va * vb),
                        LiveValue::Float64(vb) => LiveValue::Float64((va as f64) * vb),
                        LiveValue::Vec2(vb) => LiveValue::Vec2(va as f32 * vb),
                        LiveValue::Vec3(vb) => LiveValue::Vec3(va as f32 * vb),
                        LiveValue::Vec4(vb) => LiveValue::Vec4(va as f32 * vb),
                        LiveValue::Color(vb) => LiveValue::Vec4(va as f32 * Vec4::from_u32(vb)),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Float64(va) => match b {
                        LiveValue::Int64(vb) => LiveValue::Float64(va * vb as f64),
                        LiveValue::Float64(vb) => LiveValue::Float64(va * vb),
                        LiveValue::Vec2(vb) => LiveValue::Vec2(va as f32 * vb),
                        LiveValue::Vec3(vb) => LiveValue::Vec3(va as f32 * vb),
                        LiveValue::Vec4(vb) => LiveValue::Vec4(va as f32 * vb),
                        LiveValue::Color(vb) => LiveValue::Vec4(va as f32 * Vec4::from_u32(vb)),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Vec2(va) => match b {
                        LiveValue::Vec2(vb) => LiveValue::Vec2(va * vb),
                        LiveValue::Int64(vb) => LiveValue::Vec2(va * vb as f32),
                        LiveValue::Float64(vb) => LiveValue::Vec2(va * vb as f32),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Vec3(va) => match b {
                        LiveValue::Vec3(vb) => LiveValue::Vec3(va * vb),
                        LiveValue::Int64(vb) => LiveValue::Vec3(va * vb as f32),
                        LiveValue::Float64(vb) => LiveValue::Vec3(va * vb as f32),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Vec4(va) => match b {
                        LiveValue::Vec4(vb) => LiveValue::Vec4(va * vb),
                        LiveValue::Int64(vb) => LiveValue::Vec4(va * vb as f32),
                        LiveValue::Float64(vb) => LiveValue::Vec4(va * vb as f32),
                        LiveValue::Color(vb) => LiveValue::Vec4(va * Vec4::from_u32(vb)),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Color(va) => match b {
                        LiveValue::Vec4(vb) => LiveValue::Vec4(Vec4::from_u32(va) * vb),
                        LiveValue::Int64(vb) => LiveValue::Vec4(Vec4::from_u32(va) * vb as f32),
                        LiveValue::Float64(vb) => LiveValue::Vec4(Vec4::from_u32(va) * vb as f32),
                        LiveValue::Color(vb) => LiveValue::Vec4(Vec4::from_u32(va) * Vec4::from_u32(vb)),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                },
                LiveBinOp::Div => match a {
                    LiveValue::Int64(va) => match b {
                        LiveValue::Int64(vb) => LiveValue::Float64(va as f64 / vb as f64),
                        LiveValue::Float64(vb) => LiveValue::Float64((va as f64) / vb),
                        LiveValue::Vec2(vb) => LiveValue::Vec2(va as f32 / vb),
                        LiveValue::Vec3(vb) => LiveValue::Vec3(va as f32  / vb),
                        LiveValue::Vec4(vb) => LiveValue::Vec4(va as f32  / vb),
                        LiveValue::Color(vb) => LiveValue::Vec4(va as f32 / Vec4::from_u32(vb)),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Float64(va) => match b {
                        LiveValue::Int64(vb) => LiveValue::Float64(va / vb as f64),
                        LiveValue::Float64(vb) => LiveValue::Float64(va / vb),
                        LiveValue::Vec2(vb) => LiveValue::Vec2(va as f32 / vb),
                        LiveValue::Vec3(vb) => LiveValue::Vec3(va as f32 / vb),
                        LiveValue::Vec4(vb) => LiveValue::Vec4(va as f32 / vb),
                        LiveValue::Color(vb) => LiveValue::Vec4(va as f32 / Vec4::from_u32(vb)),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Vec2(va) => match b {
                        LiveValue::Vec2(vb) => LiveValue::Vec2(va / vb),
                        LiveValue::Int64(vb) => LiveValue::Vec2(va / vb as f32),
                        LiveValue::Float64(vb) => LiveValue::Vec2(va / vb as f32),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    } 
                    LiveValue::Vec3(va) => match b {
                        LiveValue::Vec3(vb) => LiveValue::Vec3(va / vb),
                        LiveValue::Int64(vb) => LiveValue::Vec3(va / vb as f32),
                        LiveValue::Float64(vb) => LiveValue::Vec3(va / vb as f32),
                        _ => return Err(LiveError::eval_error_binop_undefined_in_expression(live_error_origin!(), *index, nodes, *op, a, b))
                    }
                    LiveValue::Vec4(va) => match b {
                        LiveValue::Vec4(vb) => LiveValue::Vec4(va / vb),
                        LiveValue::Int64(vb) => LiveValue::Vec4(va / vb as f32),
                        LiveValue::Float64(vb) => LiveValue::Vec4(va / vb as f32),
                        LiveValue::Color(vb) => LiveValue::Vec4(va / Vec4::from_u32(vb)),
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
