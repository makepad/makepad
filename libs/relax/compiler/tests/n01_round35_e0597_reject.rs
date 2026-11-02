// A `&'a fn(*const dyn Tr)` (object lifetime 'static) forces the borrow of `f` to 'static: rustc E0597.
trait Tr {}
impl Tr for u8 {}
fn apply<'a>(f: &'a fn(*const dyn Tr), p: *const (dyn Tr + 'a)) { f(p); }
fn static_object(_: *const dyn Tr) {}
#[test]
fn fn_pointer_object_lifetime() {
    let f: fn(*const dyn Tr) = static_object;
    let value = 0u8;
    apply(&f, &value as *const u8 as *const dyn Tr);
}
