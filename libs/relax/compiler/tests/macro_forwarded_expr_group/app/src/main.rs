// A fragment a macro_rules arm forwards reaches a proc macro in an invisible group
// (rustc's proc_macro bridge: `OpenInvisible(MetaVar(..))` .. `CloseInvisible` is
// `Group(Delimiter::None)`), which a macro that parses with syn reads through. sqlx's
// query_as! forwards `$query:expr` to expand_query!: `"sql" + $tail` and `x as _` there --
// before: the group was a pair of real parentheses, "expected string literal" and
// "non-primitive cast" (the `as _` was not seen).
macro_rules! source {
    ($q:expr) => { fwd_mac::joined!(source = $q) };
}
macro_rules! with_tail {
    ($tail:tt) => { source!("head " + $tail) };
}
macro_rules! arg {
    ($a:expr) => { fwd_mac::inferred!($a) };
}
// what a proc macro hands back of an invisible group is read as rustc reads it
macro_rules! doubled {
    ($e:expr) => { fwd_mac::same!($e * 2) };
}
// every fragment but an `ident`, a `lifetime` and a `tt` is an invisible group, of one token
// too: const_format's `formatcp!($fmt, $arg)` reads each argument as one (wax's `sepexpr!`
// forwards through `$fmt:expr` and a block passed on as an expression)
macro_rules! expr { ($x:expr) => { fwd_mac::shape!($x) }; }
macro_rules! literal { ($x:literal) => { fwd_mac::shape!($x) }; }
macro_rules! ty { ($x:ty) => { fwd_mac::shape!($x) }; }
macro_rules! block { ($x:block) => { fwd_mac::shape!($x) }; }
macro_rules! ident { ($x:ident) => { fwd_mac::shape!($x) }; }
macro_rules! tt { ($x:tt) => { fwd_mac::shape!($x) }; }
macro_rules! args { ($f:expr $(, $x:expr)*) => { fwd_mac::shape!(($f) $(, $x)*,) }; }
macro_rules! passed_on { ($e:expr) => { $e }; }
macro_rules! twice { ($x:expr) => { passed_on! {{ fwd_mac::shape!(, $x,) }} }; }

fn main() {
    assert_eq!(source!("one"), "one");
    assert_eq!(source!("a" + "b"), "ab");
    assert_eq!(with_tail!("tail"), "head tail");
    assert!(arg!(1u8 as _));
    assert!(!arg!((1u8 as _)));
    assert!(!arg!(1u8));
    println!("forwarded expr group ok {} {}", with_tail!("tail"), arg!(2u8 as _));
    println!("handed back: {}", doubled!(1 + 1));
    println!("shapes: {} | {} | {} | {} | {} | {} | {} | {} | {}", expr!("b"), expr!(-1), literal!("b"), ty!(u8), block!({ 1 }), ident!(x), tt!("b"), args!("a{0}", "b"), twice!("b"));
}
