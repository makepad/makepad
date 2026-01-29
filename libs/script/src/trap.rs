use crate::value::*;
use crate::makepad_script_derive::*;
use std::cell::RefCell;

#[derive(Debug, Clone)]
pub struct ScriptError{
    pub in_rust: bool,
    pub message: String,
    pub origin_file: String,
    pub origin_line: u32,
    pub value: ScriptValue
}

#[derive(Debug, Clone, Copy)]
pub enum ScriptTrapOn{
    Pause,
    Return(ScriptValue),
}
use std::cell::Cell;
#[derive(Default, Debug)]
pub struct ScriptTrapInner{
    pub in_rust: bool,
    pub(crate) err: RefCell<Vec<ScriptError>>,
    pub(crate) on: Cell<Option<ScriptTrapOn>>,
    pub ip: ScriptIp,
}

pub enum ScriptTrap<'a>{
    NoTrap,
    Inner(&'a ScriptTrapInner)
}

pub use ScriptTrap::NoTrap;

impl<'a> ScriptTrap<'a>{
    pub fn pass(self)->Self{self}
}

impl ScriptTrapInner{
    pub fn pass<'a>(&'a self)->ScriptTrap{ScriptTrap::Inner(self)}
}

impl ScriptTrapInner{
    pub fn push_err(&self, err:ScriptError){
        self.err.borrow_mut().push(err)
    }
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

script_err_gen!(err_not_found);
script_err_gen!(err_not_fn);
script_err_gen!(err_not_index);
script_err_gen!(err_not_object);
script_err_gen!(err_stack_underflow);
script_err_gen!(err_stack_overflow);
script_err_gen!(err_invalid_args);
script_err_gen!(err_not_assignable);
script_err_gen!(err_unexpected); 
script_err_gen!(err_assert_fail);
script_err_gen!(err_not_impl);
script_err_gen!(err_frozen);
script_err_gen!(err_vec_frozen);
script_err_gen!(err_invalid_prop_type);
script_err_gen!(err_invalid_prop_name);
script_err_gen!(err_key_already_exists);
script_err_gen!(err_invalid_key_type);
script_err_gen!(err_vec_bound);
script_err_gen!(err_invalid_arg_type);
script_err_gen!(err_invalid_arg_name);
script_err_gen!(err_invalid_arg_count);
script_err_gen!(err_invalid_var_name);
script_err_gen!(err_not_proto);
script_err_gen!(err_type_not_registered);
script_err_gen!(err_enum_unknown_variant);
script_err_gen!(err_not_allowed_in_array);
script_err_gen!(err_user);
script_err_gen!(err_not_allowed_in_arguments);
script_err_gen!(err_array_bound);
script_err_gen!(err_wrong_type_in_apply);
script_err_gen!(err_file_system);
script_err_gen!(err_child_process);
script_err_gen!(err_too_many_paused_calls);
script_err_gen!(err_pod_type_not_extendable);
script_err_gen!(err_pod_type_not_matching);
script_err_gen!(err_pod_field_not_pod);
script_err_gen!(err_pod_array_def_incorrect);
script_err_gen!(err_pod_too_much_data);
script_err_gen!(err_pod_not_enough_data);
script_err_gen!(err_pod_invalid_field_name);
script_err_gen!(err_no_matching_shader_type);
script_err_gen!(err_opcode_not_supported_in_shader);
script_err_gen!(err_no_wgsl_conversion_available);
script_err_gen!(err_return_type_changed);
script_err_gen!(err_invalid_constructor_arg);
script_err_gen!(err_have_to_initialise_variable);
script_err_gen!(err_struct_name_not_consistent);
script_err_gen!(err_recursion_not_allowed);
script_err_gen!(err_if_else_type_different);
script_err_gen!(err_let_is_immutable);
script_err_gen!(err_opcode_not_defined_for_shader_type);
script_err_gen!(err_not_an_array);
script_err_gen!(err_index_out_of_bounds);
script_err_gen!(err_use_only_named_or_ordered_pod_fields);
script_err_gen!(err_assign_not_allowed);
script_err_gen!(err_range_requires_numbers);
