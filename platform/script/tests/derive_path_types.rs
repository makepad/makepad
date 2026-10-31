//! `#[derive(Script)]` takes every field type rustc does, in the rustc build as under the JIT:
//! path types (`crate::a::B`, `::std::..`, `self::`), with generics on any segment, at the top
//! level of a field (a top-level path once failed the rustc build only: "Unexpected field form").

use makepad_script::*;

mod inner {
    #[derive(Debug, Default, Clone, PartialEq)]
    pub struct Hold(pub u32);
    #[derive(Debug, Default)]
    pub struct Wrap<T>(pub T);
}

#[derive(Debug, Script, ScriptHook)]
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
    #[rust]
    pair: Option<(crate::inner::Hold, bool)>,
}

#[test]
fn path_typed_fields_get_their_defaults() {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let vm = &mut ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    let v = PathFields::script_new(vm);
    assert_eq!(v.a, 1.5);
    assert_eq!(v.hold, inner::Hold(0));
    assert!(v.holds.is_empty() && v.absolute.is_empty() && v.pair.is_none() && v.wrapped.0.is_none());
}
