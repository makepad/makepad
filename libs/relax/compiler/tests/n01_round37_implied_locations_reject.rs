// T2 — owner: conform (C-NLL5). Carry normalization constraints from closure
// signature/implied-bounds queries to the caller at Locations::All(span).
// rustc_borrowck/src/type_check/free_region_relations.rs:523-529;
// rustc_borrowck/src/implied_bounds.rs; Relax borrowck/implied_bounds.rs currently
// does not import normalization constraints. This fixture covers the closure
// signature-to-borrowck boundary: the raw projection may already be normalized
// before the implied-bounds query, so it does not isolate that query alone.
// Rustc rejects the annotation: independently quantified 'a cannot be 'static.
trait StaticOnly { type Output; }
impl StaticOnly for &'static () { type Output = (); }
#[test]
#[ignore = "T2 conform: signature normalization constraints require Locations"]
fn implied_bound_constraint_locations() {
    fn bad<'a>(_: &'a ()) {
        let f = |_: <&'a () as StaticOnly>::Output| {};
        f(());
    }
    bad(&());
}
