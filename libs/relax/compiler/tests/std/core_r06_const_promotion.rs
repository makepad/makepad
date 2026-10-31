// std-core repro F-STD18 (core_diff.sh): `&CONST_EXPR` in a fn returning &'static must be promoted to a
// static (rvalue static promotion); libs/futures and alloc's Arc<W: Wake> -> Waker return
// `&RawWakerVTable::new(f::<T>, ..)` this way. Relax: dangling stack ref -> SIGSEGV.
struct VT {
    a: fn(u32) -> u32,
    b: u32,
}
fn add<const K: u32>(x: u32) -> u32 {
    x + K
}
fn table<const K: u32>() -> &'static VT {
    &VT { a: add::<K>, b: K }
}
#[test]
fn a01() {
    let t = table::<5>();
    let u = table::<7>();
    println!("| {} {} {} {}", (t.a)(1), t.b, (u.a)(1), u.b);
}
