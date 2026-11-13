// A `#[cfg]` on an expression that reaches a list (array, tuple, call arguments) through an
// `$e:expr` macro fragment removes the element, as it does when written in place (ruma's
// `required_client_scopes: [#[cfg(not(feature = ..))] A, #[cfg(feature = ..)] B]`).
#[derive(Debug, PartialEq, Clone, Copy)]
enum Scope { Full, Admin }

macro_rules! scopes {
    ([$( $scope:expr ),+ $(,)?]) => { &[$($scope,)+] };
}
macro_rules! tuple_of {
    ($( $e:expr ),+) => { ($($e,)+) };
}
macro_rules! call_with {
    ($f:ident; $( $e:expr ),+) => { $f($($e,)+) };
}
fn one(a: u8) -> u8 { a }

#[test]
fn direct() {
    let a: &[Scope] = &[#[cfg(not(any()))] Scope::Full, #[cfg(any())] Scope::Admin];
    assert_eq!(a, [Scope::Full]);
}
#[test]
fn through_expr_fragment() {
    let a: &[Scope] = scopes!([#[cfg(not(any()))] Scope::Full, #[cfg(any())] Scope::Admin]);
    assert_eq!(a, [Scope::Full]);
    let b: &[Scope] = scopes!([#[cfg(any())] Scope::Full, #[cfg(not(any()))] Scope::Admin,]);
    assert_eq!(b, [Scope::Admin]);
}
#[test]
fn tuple_and_call() {
    let t: (u8, u8) = tuple_of!(1, #[cfg(any())] 2, 3);
    assert_eq!(t, (1, 3));
    assert_eq!(call_with!(one; #[cfg(any())] 9, 4), 4);
}

macro_rules! inner {
    ($( $e:expr ),+ $(,)?) => { [$($e,)+] };
}
// forwards each expression fragment to a second macro
macro_rules! outer {
    ($( $e:expr ),+ $(,)?) => { inner!($($e),+) };
}
macro_rules! sum_of {
    ($( $e:expr ),+) => { [$($e),+].iter().sum::<u32>() };
}
macro_rules! method_args {
    ($v:ident; $( $e:expr ),+) => { $v.extend_from_slice(&[$($e),+]) };
}
macro_rules! attr_true {
    ($( $e:expr ),+) => { [$($e,)+] };
}

#[test]
fn forwarded_twice() {
    let a: [u32; 2] = outer!(1, #[cfg(any())] 2, 3);
    assert_eq!(a, [1, 3]);
}
#[test]
fn without_trailing_comma_and_in_method_args() {
    assert_eq!(sum_of!(1, #[cfg(any())] 20, 3), 4);
    let mut v = vec![0u8];
    method_args!(v; #[cfg(any())] 7, 8, #[cfg(all())] 9);
    assert_eq!(v, [0, 8, 9]);
}
#[test]
fn true_cfg_and_cfg_attr_keep_the_element() {
    let a = attr_true!(#[cfg(all())] 1u8, #[cfg_attr(all(), cfg(any()))] 2u8, #[cfg_attr(any(), cfg(any()))] 3u8, #[allow(unused_parens)] (4u8));
    assert_eq!(a, [1, 3, 4]);
}
