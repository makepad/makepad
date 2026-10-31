//~ msg: mismatched types
// Normalizing an associated input must still reject a genuinely recursive
// output: T = Wrapper<T>.
trait Project { type Out; }
struct Wrapper<T>(T);
impl<T> Project for Wrapper<T> { type Out = Wrapper<T>; }
fn call<T, F: FnOnce(<Wrapper<T> as Project>::Out) -> T>(_: F) {}
fn main() { call(|x| x); }
