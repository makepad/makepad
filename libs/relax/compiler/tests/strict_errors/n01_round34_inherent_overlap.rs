struct S<T>(T);
impl<T> S<T> { fn f() {} }
impl S<u32> { fn f() {} }
fn main() {}
