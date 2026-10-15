trait Item { type Out; }
struct S;
impl Item for S { type Out = u32; }
trait Tr {}
impl Tr for <S as Item>::Out {}
impl Tr for u32 {}
fn main() {}
