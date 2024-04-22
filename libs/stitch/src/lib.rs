mod aliased_box;
mod code;
mod compile;
mod config;
mod const_expr;
mod data;
mod decode;
mod downcast;
mod elem;
mod engine;
mod error;
mod extern_;
mod extern_ref;
mod extern_val;
mod func;
mod func_ref;
mod global;
mod instance;
mod exec;
mod limits;
mod linker;
mod mem;
mod module;
mod ops;
mod ref_;
mod stack;
mod store;
mod table;
mod trap;
mod val;
mod validate;
mod wrap;

pub use self::{
    decode::DecodeError,
    engine::Engine,
    error::Error,
    extern_ref::ExternRef,
    extern_val::{ExternType, ExternVal},
    func::{Func, FuncType},
    func_ref::FuncRef,
    global::{Global, GlobalError, GlobalType, Mut},
    instance::Instance,
    limits::Limits,
    linker::{InstantiateError, Linker},
    mem::{Mem, MemError, MemType},
    module::{Module, ModuleExports},
    ref_::{Ref, RefType},
    stack::Stack,
    store::Store,
    table::{Table, TableError, TableType},
    val::{Val, ValType},
};
