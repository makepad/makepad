// rustc_hir_typeck/src/callee.rs:610-615: each call instantiates the binder.
fn call<F: for<'a> Fn(&'a u32) -> &'a u32>(f: F, x: &u32) -> u32 { *f(x) }
fn identity(x: &u32) -> &u32 { x }
#[test]
fn higher_ranked_parameter_call() { assert_eq!(call(identity, &9), 9); }
#[test]
fn function_item_iterator_projection() {
    let names = vec![String::from("a"), String::from("b")];
    let parts = names.iter().map(String::as_str).collect::<Vec<_>>();
    assert_eq!(parts, vec!["a", "b"]);
}
#[test]
fn nested_iterator_callable_regions() {
    let rows = vec![Some(String::from("a")), None, Some(String::from("b"))];
    let parts = rows.iter().filter_map(|row| row.as_deref()).collect::<Vec<_>>();
    assert_eq!(parts, vec!["a", "b"]);
}
enum Selection<'a> { Value(&'a str) }
impl<'a> From<&'a str> for Selection<'a> { fn from(x: &'a str) -> Self { Self::Value(x) } }
fn choose<'a>(input: impl Into<Selection<'a>>) -> usize {
    match input.into() { Selection::Value(x) => x.len() }
}
#[test]
fn apit_lifetime_is_early_bound() { assert_eq!(choose("abc"), 3); }
struct Holder<'a>(&'a str);
impl Holder<'_> { fn mapped(&self) -> usize { [1u32, 2].iter().map(|v| *v as usize + self.0.len()).sum() } }
#[test]
fn closure_owner_keeps_anonymous_impl_lifetime() { assert_eq!(Holder("a").mapped(), 5); }
trait Requests { type Request<'a>; fn len(req: Self::Request<'_>) -> usize; }
struct Client;
impl Requests for Client { type Request<'a> = &'a str; fn len(req: Self::Request<'_>) -> usize { req.len() } }
#[test]
fn self_gat_keeps_own_lifetime() { assert_eq!(Client::len("abcd"), 4); }
#[test]
fn higher_ranked_object_coercion() {
    let f: &dyn for<'a> Fn(&'a u32) -> &'a u32 = &identity;
    assert_eq!(*f(&17), 17);
}

#[test]
fn higher_ranked_fn_pointer_builtin_marker() {
    fn identity(s: &str) -> &str { s }
    fn is_ptr<T: core::ops::FnPtr>(_: T) {}
    let f: for<'a> fn(&'a str) -> &'a str = identity;
    is_ptr(f);
}
