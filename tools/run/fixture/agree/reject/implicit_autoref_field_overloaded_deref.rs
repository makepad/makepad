// rejects: implicit autoref creates a reference to the dereference of a raw pointer
pub struct W { pub b: Box<(u8, u8)> }
pub unsafe fn f(raw: *const W) -> *const u8 { unsafe { &raw const (*(*raw).b.as_ref()).0 ; &raw const (*raw).s().0 } }
impl W { pub fn s(&self) -> &(u8, u8) { &self.b } }
pub unsafe fn g(raw: *const String) -> usize { unsafe { (*raw).len() } }
