// Normalization constraints from the closure signature reach the caller at Locations::All(span)
// (rustc_borrowck/src/type_check/free_region_relations.rs:523-529, implied_bounds.rs).
// Rustc rejects the annotation: independently quantified 'a cannot be 'static.
trait StaticOnly { type Output; }
impl StaticOnly for &'static () { type Output = (); }
#[test]
fn implied_bound_constraint_locations() {
    fn bad<'a>(_: &'a ()) {
        let f = |_: <&'a () as StaticOnly>::Output| {};
        f(());
    }
    bad(&());
}
