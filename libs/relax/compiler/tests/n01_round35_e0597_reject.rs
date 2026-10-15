// T3: fn-item coercion E0597 in makepad test; owner: conform.
// OA round-33 report, L4' fn-pointer object-lifetime regression.
trait Tr {}
impl Tr for u8 {}
fn apply<'a>(f: &'a fn(*const dyn Tr), p: *const (dyn Tr + 'a)) { f(p); }
fn static_object(_: *const dyn Tr) {}
#[test]
#[ignore = "T3: makepad test E0597; conform"]
fn fn_pointer_object_lifetime() {
    let f: fn(*const dyn Tr) = static_object;
    let value = 0u8;
    apply(&f, &value as *const u8 as *const dyn Tr);
}
