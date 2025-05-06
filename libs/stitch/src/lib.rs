mod aliasable_box;
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
mod exec;
mod extern_;
mod extern_ref;
mod extern_val;
mod func;
mod func_ref;
mod global;
mod instance;
mod into_host_func;
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

pub use self::{
    decode::DecodeError,
    engine::Engine,
    error::Error,
    extern_ref::ExternRef,
    extern_val::{ExternType, ExternVal},
    func::{Func, FuncError, FuncType},
    func_ref::FuncRef,
    global::{Global, GlobalError, GlobalType, Mut},
    instance::{Instance, InstanceExports},
    limits::Limits,
    linker::{InstantiateError, Linker},
    mem::{Mem, MemError, MemType},
    module::{Module, ModuleExports, ModuleImports},
    ref_::{Ref, RefType},
    store::Store,
    table::{Table, TableError, TableType},
    val::{Val, ValType},
};
