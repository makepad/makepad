use crate::value::*;

#[derive(Debug, Clone, Copy)]
pub struct ScriptError{
    pub in_rust: bool,
    pub value: ScriptValue
}

#[derive(Debug, Clone, Copy)]
pub enum ScriptTrapOn{
    Pause,
    Return(ScriptValue),
}
use std::cell::Cell;
#[derive(Default, Debug)]
pub struct ScriptTrap{
    pub in_rust: bool,
    pub(crate) err: Cell<Option<ScriptError>>,
    pub(crate) on: Cell<Option<ScriptTrapOn>>,
    pub ip: ScriptIp,
}


impl ScriptTrap{
    pub fn ip(&self)->u32{
        self.ip.index
    }
    pub fn goto(&mut self, wh:u32){
        self.ip.index = wh;
    }
    pub fn goto_rel(&mut self, wh:u32){
        self.ip.index += wh;
    }
    pub fn goto_next(&mut self){
        self.ip.index += 1;
    }
}

macro_rules! err_fwd{
    ($name:ident)=>{
        pub fn $name(&self)->ScriptValue{self.err(ScriptValue::$name(self.ip))}
    }
}
impl ScriptTrap{
    pub fn err(&self, err:ScriptValue)->ScriptValue{
        self.err.set(Some(ScriptError{
            in_rust:self.in_rust,
            value:err
        }));
        err
    }
    err_fwd!(err_not_found);
    err_fwd!(err_not_fn);
    err_fwd!(err_not_index);
    err_fwd!(err_not_object);
    err_fwd!(err_stack_underflow);
    err_fwd!(err_stack_overflow);
    err_fwd!(err_invalid_args);
    err_fwd!(err_not_assignable);
    err_fwd!(err_unexpected);
    err_fwd!(err_assert_fail);
    err_fwd!(err_not_impl);
    err_fwd!(err_frozen);
    err_fwd!(err_vec_frozen);
    err_fwd!(err_invalid_prop_type);
    err_fwd!(err_invalid_prop_name);
    err_fwd!(err_key_already_exists);
    err_fwd!(err_invalid_key_type);
    err_fwd!(err_vec_bound);
    err_fwd!(err_invalid_arg_type);
    err_fwd!(err_invalid_arg_name);
    err_fwd!(err_invalid_arg_count);
    err_fwd!(err_invalid_var_name);
    err_fwd!(err_not_proto);
    err_fwd!(err_type_not_registered);
    err_fwd!(err_enum_unknown_variant);
    err_fwd!(err_not_allowed_in_array);
    err_fwd!(err_user);
    err_fwd!(err_not_allowed_in_arguments);
    err_fwd!(err_array_bound);
    err_fwd!(err_wrong_type_in_apply);
    err_fwd!(err_file_system);
    err_fwd!(err_child_process);
    err_fwd!(err_too_many_paused_calls);
    err_fwd!(err_pod_type_not_extendable);
    err_fwd!(err_pod_type_not_matching);
    err_fwd!(err_pod_field_not_pod);
    err_fwd!(err_pod_array_def_incorrect);
    err_fwd!(err_pod_too_much_data);
    err_fwd!(err_pod_not_enough_data);
    err_fwd!(err_pod_invalid_field_name);
    err_fwd!(err_no_matching_shader_type);
    err_fwd!(err_opcode_not_supported_in_shader);
    err_fwd!(err_no_wgsl_conversion_available);
    err_fwd!(err_return_type_changed);
    err_fwd!(err_invalid_constructor_arg);
    err_fwd!(err_have_to_initialise_variable);
    err_fwd!(err_struct_name_not_consistent);
    err_fwd!(err_recursion_not_allowed);
    err_fwd!(err_if_else_type_different);
    err_fwd!(err_let_is_immutable);
    err_fwd!(err_opcode_not_defined_for_shader_type);
    err_fwd!(err_not_an_array);
    err_fwd!(err_index_out_of_bounds);
    err_fwd!(err_use_only_named_or_ordered_pod_fields);
    err_fwd!(err_assign_not_allowed);
}

