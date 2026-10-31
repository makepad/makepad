// std::any::type_name as rustc prints it: items in a fn body under that fn, methods through
// their impl's self type, closures as `fn::{{closure}}` per nesting level, `dyn Fn(A) -> R`
// sugar (std-internal module paths like alloc::vec are the std's own layout: not tested here)
use std::any::{type_name, type_name_of_val};
trait T {}
struct S<A>(A);
struct Bar<A>(A);
impl<A> Bar<A> {
    fn foo() -> &'static str {
        fn f() {}
        type_name_of_val(&f)
    }
}
fn ret_closure() -> impl Fn(u8) { |_x| {} }

#[test]
fn paths() {
    struct L;
    let c = || 1;
    let d = || { let e = || 2; e };
    let mut v = Vec::new();
    v.push(type_name::<L>().to_string());
    v.push(type_name_of_val(&c).to_string());
    v.push(type_name_of_val(&d()).to_string());
    v.push(type_name::<S<u8>>().to_string());
    v.push(type_name::<Option<fn(u8) -> u16>>().to_string());
    v.push(type_name_of_val(&ret_closure()).to_string());
    v.push(type_name_of_val(&paths).to_string());
    v.push(Bar::<u8>::foo().to_string());
    let base = module_path!();
    let want: Vec<String> = [
        "{m}::paths::L", "{m}::paths::{{closure}}", "{m}::paths::{{closure}}::{{closure}}", "{m}::S<u8>",
        "core::option::Option<fn(u8) -> u16>", "{m}::ret_closure::{{closure}}", "{m}::paths", "{m}::Bar<_>::foo::f",
    ].iter().map(|x| x.replace("{m}", base)).collect();
    assert_eq!(v, want);
    let fnty = type_name::<dyn FnMut() -> bool>();
    assert!(fnty.starts_with("dyn core::ops::") && fnty.ends_with("FnMut() -> bool"), "{}", fnty);
}
