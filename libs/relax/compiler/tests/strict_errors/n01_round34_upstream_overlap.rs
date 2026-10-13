trait Tr {}
struct S;
impl<T: std::fmt::Display> Tr for T {}
impl Tr for Vec<S> {}
fn main() {}
