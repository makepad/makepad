//! derive(Script)'s field parser rejects a fn-pointer field type: rustc reports the derive's
//! `compile_error!` ("Unexpected field form") and the missing impls at their uses; `makepad check`
//! must report the same (it once passed a struct rustc's derive rejected, 10-25 SandboxView).
use makepad_script::*;

#[derive(Script, ScriptHook)]
struct Rejected {
    #[live(1.5)]
    a: f64,
    #[rust]
    callback: fn(u32) -> u32,
}

fn main() {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let vm = &mut ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    let v = Rejected::script_new(vm);
    println!("{} {}", v.a, (v.callback)(1));
}
