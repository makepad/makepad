//! Path field types (`crate::a::B`, `::std::..`, `self::`, generic segments) derive in rustc's
//! proc-macro build as in relax's linked copy: both check clean and print the defaults.
use makepad_script::*;

mod inner {
    #[derive(Debug, Default, Clone, PartialEq)]
    pub struct Hold(pub u32);
    #[derive(Debug, Default)]
    pub struct Wrap<T>(pub T);
}

#[derive(Script, ScriptHook)]
struct PathFields {
    #[live(1.5)]
    a: f64,
    #[rust]
    hold: crate::inner::Hold,
    #[rust]
    holds: Vec<crate::inner::Hold>,
    #[rust]
    wrapped: inner::Wrap<Option<(String, String)>>,
    #[rust]
    absolute: ::std::collections::BTreeMap<u32, self::inner::Hold>,
}

fn main() {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let vm = &mut ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    let v = PathFields::script_new(vm);
    println!("{} {:?} {} {} {}", v.a, v.hold, v.holds.len(), v.wrapped.0.is_none(), v.absolute.len());
}
