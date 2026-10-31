#![allow(dead_code, refining_impl_trait)]
trait Tr {
    fn get<'a, 'b: 'a>(x: &'a u32, y: &'b u32) -> impl Copy + 'a;
}
struct S;
impl Tr for S {
    fn get<'a, 'b: 'a>(_: &'a u32, y: &'b u32) -> &'b u32 { y }
}
