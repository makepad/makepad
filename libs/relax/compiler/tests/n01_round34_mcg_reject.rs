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
