use crate::value::*;
use crate::makepad_script_derive::*;
use std::cell::RefCell;

#[derive(Debug, Clone)]
pub struct ScriptError{
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
    pub(crate) err: RefCell<Vec<ScriptError>>,
    pub(crate) on: Cell<Option<ScriptTrapOn>>,
    pub ip: ScriptIp,
}

#[derive(Clone, Copy)]
pub enum ScriptTrap<'a>{
    NoTrap,
    Inner(&'a ScriptTrapInner)
}

pub use ScriptTrap::NoTrap;

impl<'a> ScriptTrap<'a>{
    pub fn pass(self)->Self{self}
}

impl ScriptTrapInner{
    pub fn pass<'a>(&'a self)->ScriptTrap<'a>{ScriptTrap::Inner(self)}
}

impl ScriptTrapInner{
    pub fn push_err(&self, value:ScriptValue,  message:String, origin_file:String, origin_line:u32)->ScriptValue{
        self.err.borrow_mut().push(ScriptError{
            value,
            message,
            origin_file,
            origin_line
        });
        value
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

// Consolidated error macros (19 total, down from 56)
script_err_gen!(script_err_not_found);      // lookup failures
script_err_gen!(script_err_type_mismatch);  // wrong type for operation
script_err_gen!(script_err_wrong_value);    // expected different kind
script_err_gen!(script_err_out_of_bounds);  // index/bounds errors
script_err_gen!(script_err_immutable);      // cannot modify
script_err_gen!(script_err_stack);          // stack errors
script_err_gen!(script_err_invalid_args);   // argument errors
script_err_gen!(script_err_not_allowed);    // operation not allowed
script_err_gen!(script_err_inconsistent);   // types don't match across branches
script_err_gen!(script_err_not_impl);       // not implemented
script_err_gen!(script_err_unexpected);     // catch-all
script_err_gen!(script_err_assert_fail);    // assertions
script_err_gen!(script_err_user);           // user-generated
script_err_gen!(script_err_pod);            // all pod errors
script_err_gen!(script_err_shader);         // all shader errors
script_err_gen!(script_err_unknown_type);   // type not registered
script_err_gen!(script_err_duplicate);      // key already exists
script_err_gen!(script_err_io);             // file system, child process
script_err_gen!(script_err_limit);          // resource limits
