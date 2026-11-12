//! macro_rules matching follows every matcher position at once, as rustc's mbe macro_parser
//! does: where a token can end a repetition or continue it, both ways go on, and a fragment
//! is parsed only when it is the one way on (relax issue 21).

// `$(,)?` inside a repetition that starts with `,`: after `a` the `,` begins the optional
// `$(,)?` or the next repetition
macro_rules! names {
    ($(, $x:ident $(,)?)*) => { concat!($(stringify!($x), " "),*) };
}

// a parameter list with an optional trailing comma per entry
macro_rules! params {
    ($name:ident $(, $p:ident: $t:ty $(,)?)*) => {
        fn $name($($p: $t),*) -> usize { 0 $(+ { let _ = $p; 1 })* }
    };
}
params!(none);
params!(one, a: u8);
params!(two, a: u8, b: &str,);
params!(three, a: u8, b: u16, c: u32);

macro_rules! sum {
    ($($x:expr),* $(,)?) => { 0 $(+ $x)* };
}
macro_rules! idents {
    ($($x:ident);+ $(;)?) => { [$(stringify!($x)),+] };
}
macro_rules! with_default {
    ($a:ident $(= $v:expr)?) => {{ let mut $a = 0; $($a = $v;)? $a }};
}
macro_rules! alternatives {
    ($($x:literal)|+) => { [$($x),+] };
}
macro_rules! table {
    ($($k:ident: [$($v:expr),*]);* $(;)?) => { [$((stringify!($k), 0 $(+ $v)*)),*] };
}
// a repetition ended by a token its fragment cannot begin with
macro_rules! until_semi {
    ($($x:ident)* ; $e:expr) => { (0 $(+ { let _ = stringify!($x); 1 })*, $e) };
}
// the separator, then the same token after the repetition
macro_rules! sep_then_last {
    ($($x:ident),* ; $last:ident) => { [$(stringify!($x),)* stringify!($last)] };
}
// an optional token that is also what follows it: `a` alone ends the `?` at once
macro_rules! opt_same {
    ($(a)? a) => { "one" };
    ($(a)? a a) => { "two" };
}
// the first arm fails quietly where its fragment cannot begin with the token
macro_rules! kind {
    ($e:expr) => { "expr" };
    (struct $i:ident) => { "struct" };
}
// a visibility that matches nothing
macro_rules! vis_fn {
    ($v:vis fn $n:ident) => { $v fn $n() -> u8 { 1 } };
}
vis_fn!(fn private_one);
vis_fn!(pub fn public_one);
vis_fn!(pub(crate) fn crate_one);
// token-tree munching
macro_rules! count_tts {
    () => { 0usize };
    ($head:tt $($rest:tt)*) => { 1usize + count_tts!($($rest)*) };
}
// nested repetitions with separators at both levels and a trailing separator outside
macro_rules! rows {
    ($([$($c:expr),+]),+ $(,)?) => { [$(0 $(+ $c)+),+] };
}
// a fragment followed by a literal token and an optional group after
macro_rules! typed {
    ($t:ty => $e:expr $(; $more:expr)*) => { { let v: $t = $e; v $(+ $more)* } };
}
// a forwarded fragment is one opaque group of its kind: a `ty` does not begin an `expr`
macro_rules! expr_or_ty {
    ($e:expr) => { "expr" };
    ($t:ty) => { "ty" };
}
macro_rules! forward_ty {
    ($t:ty) => { expr_or_ty!($t) };
}
macro_rules! forward_expr {
    ($e:expr) => { expr_or_ty!($e) };
}
// `const` does not begin an `expr` before edition 2024
macro_rules! const_block {
    ($e:expr) => { "expr" };
    (const $b:block) => { "const block" };
}

#[test]
fn matcher_positions() {
    assert_eq!(names!(, a), "a ");
    assert_eq!(names!(, a, b), "a b ");
    assert_eq!(names!(, a, b,), "a b ");
    assert_eq!(names!(), "");
    assert_eq!((none(), one(1), two(1, "x"), three(1, 2, 3)), (0, 1, 2, 3));
    assert_eq!((sum!(), sum!(1), sum!(1, 2), sum!(1, 2,)), (0, 1, 3, 3));
    assert_eq!(idents!(a; b; c;), ["a", "b", "c"]);
    assert_eq!(idents!(a), ["a"]);
    assert_eq!((with_default!(x), with_default!(y = 4)), (0, 4));
    assert_eq!(alternatives!(1 | 2 | 3), [1, 2, 3]);
    assert_eq!(table!(a: [1, 2]; b: []; c: [3];), [("a", 3), ("b", 0), ("c", 3)]);
    assert_eq!(until_semi!(a b c ; 7), (3, 7));
    assert_eq!(until_semi!(; 1), (0, 1));
    assert_eq!(sep_then_last!(a, b ; c), ["a", "b", "c"]);
    assert_eq!(sep_then_last!(; c), ["c"]);
    assert_eq!((opt_same!(a), opt_same!(a a), opt_same!(a a a)), ("one", "one", "two"));
    assert_eq!((kind!(1 + 2), kind!(struct S)), ("expr", "struct"));
    assert_eq!(private_one() + public_one() + crate_one(), 3);
    assert_eq!(count_tts!(a (b c) [d] {e f} , ;), 6);
    assert_eq!(rows!([1, 2], [3], [4, 5, 6],), [3, 3, 15]);
    assert_eq!((typed!(u8 => 1), typed!(u32 => 1; 2; 3)), (1, 6));
    assert_eq!((forward_ty!(Vec<u8>), forward_ty!(u8), forward_expr!(1 + 1), forward_expr!(x)), ("ty", "ty", "expr", "expr"));
    assert_eq!((const_block!(const { 1 }), const_block!(-1)), ("const block", "expr"));
}
