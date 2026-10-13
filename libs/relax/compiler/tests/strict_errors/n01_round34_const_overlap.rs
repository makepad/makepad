trait Tr {}
impl<const N: usize> Tr for [u32; N] {}
impl Tr for [u32; 2] {}
fn main() {}
