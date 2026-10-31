// D7 / front-bck audit 1.5: rigid aliases in borrowck (nll.rs params_in: type tests for rigid
// projections; rty.rs skel_into: region skeleton for rigid opaques), compared with rustc.
trait Tr { type Item; }
impl Tr for u8 { type Item = u16; }
fn need_static<U: 'static>(_: U) {}

// T::Item is rigid here; T: 'static proves it by the projection-outlives rule.
fn by_args<T: Tr + 'static>(x: T::Item) { need_static(x); }
// ... and a where-clause on the projection itself.
fn by_bound<T: Tr>(x: T::Item) where T::Item: 'static { need_static(x); }

fn mk<'a>(x: &'a u8) -> impl Sized + 'a { x }
// A rigid opaque carries its captured region: the returned value borrows `x` only.
fn pass_through<'a, 'b>(x: &'a u8, _y: &'b u8) -> impl Sized + 'a { mk(x) }

#[test]
fn rigid_projection_outlives() {
    by_args::<u8>(1);
    by_bound::<u8>(2);
}

#[test]
fn rigid_opaque_regions() {
    let a = 1u8;
    let r;
    {
        let b = 2u8;
        r = pass_through(&a, &b);
    }
    let _ = r;
}
