// rustc ui closure-expected-type/expect-region-supply-region.rs expect_free_supply_named: the
// annotation names the parent's 'x where the expected input region is a local borrow's (E0597).
// The annotated input's region is the closure signature's own, ascribed onto by the annotation.
fn closure_expecting_free<'a, F>(_: &'a u32, _: F) where F: FnMut(&'a u32) {}
fn expect_free_supply_named<'x>() {
    let x = 22;
    let mut f: Option<&u32> = None;
    closure_expecting_free(&x, |y: &'x u32| {
        f = Some(y);
    });
    drop(f);
}
fn main() {
    expect_free_supply_named();
}
