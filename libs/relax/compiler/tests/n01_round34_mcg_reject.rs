// rejects: E0770
// rejects: E0747
// rejects: E0308
// rejects: E0128
// rejects: generic parameters may not be used in const operations
// rejects: generic parameters may not be used in enum discriminant values
// rejects-at: 23:46 E0747
// (rustc: E0747 "unresolved item provided when a constant was expected" at `id::<N>()`, the N of a
// const operation resolving to an error; E0308 at the default "bad" of a u8 const parameter, whose
// anonymous constant is type-checked as a body owner. A failed discriminant (`E`) stops neither.)
#![allow(dead_code)]
struct A<const N: usize>;
fn arithmetic<const N: usize>() -> [u8; N + 1] { loop {} }
fn type_param<T>() -> [u8; std::mem::size_of::<T>()] { loop {} }
fn nested_braces<const N: usize>() -> A<{ { N } }> { loop {} }
fn repeat_const<const N: usize>() { let _ = [0; N + 1]; }
struct DefaultOp<const N: usize, const M: usize = { N + 1 }>;
struct Forward<const N: usize = M, const M: usize = 2>;
struct ParamType<T, const N: T>(T);
trait Tr { fn f() -> [u8; std::mem::size_of::<Self>()]; }
enum E<const N: usize> { A = N as isize }
const fn id<const N: usize>() -> usize { N }
fn nested_arg<const N: usize>() -> [u8; id::<N>()] { loop {} }
struct BadTypeDefault<const N: u8 = "bad">;
