#![allow(dead_code, unused_braces)]
struct A<const N: usize>;
struct Defaults<const N: usize = 2, const M: usize = N>;
fn bare<const N: usize>(x: A<{ N }>) -> A<N> { x }
fn repeat_type<T>() -> usize { [0u8; std::mem::size_of::<*const T>()].len() }
fn inline<const N: usize>() -> usize { const { N + 1 } }
fn shadow<const N: usize>() -> usize { let a = [0u8; { const N: usize = 3; N + 1 }]; a.len() }
struct S;
impl S { const N: usize = 4; fn self_alias() -> [u8; Self::N] { [0; Self::N] } }
#[test] fn accepted_const_scopes() {
    let _: A<3> = bare(A::<3>);
    assert_eq!(repeat_type::<u32>(), 8);
    assert_eq!(inline::<3>(), 4);
    assert_eq!(shadow::<2>(), 4);
    assert_eq!(S::self_alias().len(), 4);
}
