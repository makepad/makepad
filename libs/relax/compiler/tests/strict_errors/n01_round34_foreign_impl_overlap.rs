// Conflicts with core's upstream blanket impl<T> From<T> for T.
struct S;
impl From<S> for S { fn from(value: S) -> S { value } }
fn main() {}
