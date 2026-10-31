// `let`-extended temporaries (`let r = R { c: &D(..) };`, `let x = &D(..);`) drop in reverse
// declaration order together with the block's locals: after the locals declared later, before
// the `let`'s own bindings and earlier locals (rustc schedules the temporary's drop when the
// initializer makes it). Relax dropped them after every local of the block (silent order bug,
// tests/ui closures/2229_closure_analysis/preserve_field_drop_order2.rs). Sequences = rustc.
use std::cell::RefCell;
thread_local! { static LOG: RefCell<Vec<String>> = RefCell::new(Vec::new()); }
fn take() -> String { LOG.with(|l| std::mem::take(&mut *l.borrow_mut())).join(",") }
struct D(&'static str);
impl Drop for D { fn drop(&mut self) { LOG.with(|l| l.borrow_mut().push(self.0.to_string())); } }
struct R<'a> { c: &'a D, d: &'a D }

fn blocks() {
    let _a = D("a");
    let r = R { c: &D("ext1"), d: &D("ext1b") };
    let _b = D("b");
    let x = &D("ext2");
    let _c = D("c");
    {
        let _i = D("i");
        let y = &D("ext3");
        let _j = D("j");
        let _ = y.0;
    }
    let _ = (r.c.0, r.d.0, x.0);
}
fn early(n: u32) -> u32 {
    let _a = D("ea");
    let r = R { c: &D("e1"), d: &D("e2") };
    let _b = D("eb");
    if n == 1 { return 1; }
    let x = &D("e3");
    let _c = D("ec");
    if n == 2 { return 2; }
    let _ = (r.c.0, r.d.0, x.0);
    0
}
fn in_loop() {
    let mut i = 0;
    loop {
        let _l = D("l");
        let y = &D("lt");
        let _m = D("m");
        i += 1;
        if i == 2 { break; }
        let _ = y.0;
    }
}
fn unwinds() {
    let _a = D("ua");
    let _x = &D("ut");
    let _b = D("ub");
    panic!("p");
}

#[test]
fn extended_temps_in_declaration_order() {
    blocks();
    assert_eq!(take(), "j,ext3,i,c,ext2,b,ext1b,ext1,a");
    let mut s = Vec::new();
    for n in 0..3 { early(n); s.push(take()); }
    assert_eq!(s, ["ec,e3,eb,e2,e1,ea", "eb,e2,e1,ea", "ec,e3,eb,e2,e1,ea"]);
    in_loop();
    assert_eq!(take(), "m,lt,l,m,lt,l");
    std::panic::set_hook(Box::new(|_| {}));
    let _ = std::panic::catch_unwind(|| unwinds());
    assert_eq!(take(), "ub,ut,ua");
    {
        let (a, b) = (&D("t1"), D("v1"));
        let s: &[D] = &[D("s1"), D("s2")];
        let _k = D("k");
        let _ = (a.0, b.0, s.len());
    }
    assert_eq!(take(), "k,s1,s2,v1,t1");
}
