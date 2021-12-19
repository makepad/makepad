pub use {
    std::{
        any::TypeId,
    },
    makepad_shader_compiler::makepad_live_compiler::*,
    crate::{
        cx::Cx,
        event::Event,
        animator::Animator,
        live_traits::*,
    }
};


#[derive(Debug)]
pub enum LiveEval {
    Float(f64),
    Int(i64),
    Bool(bool),
    Void
}

pub fn live_eval(cx: &mut Cx, start: usize, apply_from: ApplyFrom, index: &mut usize, nodes: &[LiveNode]) -> LiveEval {
    match &nodes[*index].value {
        LiveValue::Float(v) => {
            *index += 1;
            return LiveEval::Float(*v);
        }
        LiveValue::Int(v) => {
            *index += 1;
            return LiveEval::Int(*v);
        }
        LiveValue::Bool(v) => {
            *index += 1;
            return LiveEval::Bool(*v);
        }
        LiveValue::Id(id) => { // look it up from start on up
            *index += 1;
            if let Some(index) = nodes.scope_up_by_name(start - 1, *id) {
                // found ok now what. it depends on the type of the thing here
                return match &nodes[index].value {
                    LiveValue::Float(val) => LiveEval::Float(*val),
                    LiveValue::Int(val) => LiveEval::Int(*val),
                    LiveValue::Bool(val) => LiveEval::Bool(*val),
                    LiveValue::Expr => {
                        LiveEval::Void
                    }
                    LiveValue::Array => { // got an animation track. select the last value
                        if let Some(index) = Animator::last_keyframe_value_from_array(index, nodes) {
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
                        cx.apply_error_wrong_value_in_expression(live_error_origin!(), apply_from, index, nodes, "Id");
                        LiveEval::Void
                    }
                }
            }
            else {
                cx.apply_error_cant_find_target(live_error_origin!(), apply_from, *index, nodes, *id);
                LiveEval::Void
            }
        },
        LiveValue::ExprUnOp(op) => {
            *index += 1;
            let a = live_eval(cx, start, apply_from, index, nodes);
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
                cx.apply_error_unop_undefined_in_expression(live_error_origin!(), apply_from, *index, nodes, *op, a);
            }
            ret
        }
        LiveValue::ExprCall {ident, args} => {
            *index += 1;
            for _ in 0..*args {
                live_eval(cx, start, apply_from, index, nodes);
            }
            cx.apply_error_expression_call_not_implemented(live_error_origin!(), apply_from, *index, nodes, *ident, *args);
            LiveEval::Void
        }
        LiveValue::ExprBinOp(op) => {
            *index += 1;
            let a = live_eval(cx, start, apply_from, index, nodes);
            let b = live_eval(cx, start, apply_from, index, nodes);
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
                        _ => LiveEval::Void
                    }
                    LiveEval::Float(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Float(va + vb as f64),
                        LiveEval::Float(vb) => LiveEval::Float(va + vb),
                        _ => LiveEval::Void
                    }
                    _ => LiveEval::Void
                },
                LiveBinOp::Sub => match a {
                    LiveEval::Int(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Int(va - vb),
                        LiveEval::Float(vb) => LiveEval::Float((va as f64) - vb),
                        _ => LiveEval::Void
                    }
                    LiveEval::Float(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Float(va - vb as f64),
                        LiveEval::Float(vb) => LiveEval::Float(va - vb),
                        _ => LiveEval::Void
                    }
                    _ => LiveEval::Void
                },
                LiveBinOp::Mul => match a {
                    LiveEval::Int(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Int(va * vb),
                        LiveEval::Float(vb) => LiveEval::Float((va as f64) * vb),
                        _ => LiveEval::Void
                    }
                    LiveEval::Float(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Float(va * vb as f64),
                        LiveEval::Float(vb) => LiveEval::Float(va * vb),
                        _ => LiveEval::Void
                    }
                    _ => LiveEval::Void
                },
                LiveBinOp::Div => match a {
                    LiveEval::Int(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Float(va as f64 / vb as f64),
                        LiveEval::Float(vb) => LiveEval::Float((va as f64) / vb),
                        _ => LiveEval::Void
                    }
                    LiveEval::Float(va) => match b {
                        LiveEval::Int(vb) => LiveEval::Float(va / vb as f64),
                        LiveEval::Float(vb) => LiveEval::Float(va / vb),
                        _ => LiveEval::Void
                    }
                    _ => LiveEval::Void
                },
            };
            if let LiveEval::Void = ret {
                cx.apply_error_binop_undefined_in_expression(live_error_origin!(), apply_from, *index, nodes, *op, a, b);
            }
            ret
        }
        _ => {
            cx.apply_error_wrong_value_in_expression(live_error_origin!(), apply_from, *index, nodes, "f32");
            *index = nodes.skip_node(*index);
            LiveEval::Void
        }
    }
}
