// Readers of fields.rs's structs, in another file. An edit there changes no text here.
use crate::fields::*;
use std::hint::black_box;

macro_rules! read_by_macro {
    ($f:expr) => {
        $f.u[0] + $f.u.len() as u64 * 10
    };
}

#[inline(never)]
fn free(f: &Frame) -> u64 {
    f.u[0] + f.u.len() as u64 * 10
}

#[inline(never)]
fn pattern(f: &Frame) -> u64 {
    let Frame { u, .. } = f;
    u[0] + u.len() as u64 * 10
}

#[inline(never)]
fn closure(f: &Frame) -> u64 {
    let g = || f.u[0] + f.u.len() as u64 * 10;
    black_box(g)()
}

#[inline(never)]
fn generic<T: Default>(f: &Frame) -> u64 {
    let _ = T::default();
    f.u[0] + f.u.len() as u64 * 10
}

#[inline(never)]
fn inlined(f: &Frame) -> u64 {
    f.tiny()
}

#[inline(never)]
fn by_macro(f: &Frame) -> u64 {
    read_by_macro!(f)
}

#[inline(never)]
fn by_offset(x: &OffsetRead) -> u64 {
    let at = core::mem::offset_of!(OffsetRead, u);
    unsafe { *(x as *const OffsetRead as *const u8).add(at).cast::<u32>() as u64 }
}

#[inline(never)]
fn nested(h: &Holds) -> u64 {
    h.frame.u[0] + h.frame.u.len() as u64 * 10
}

#[inline(never)]
fn by_dyn(r: &dyn Read) -> u64 {
    r.read()
}

#[inline(never)]
fn same_size(x: &SameSize) -> u64 {
    x.s as u64 * 10 + x.t
}

#[inline(never)]
fn two_kinds(x: &TwoKinds) -> u64 {
    x.a as u64 * 10 + x.b as u64
}

#[inline(never)]
fn order(x: &Order) -> u64 {
    x.a * 10 + x.b
}

#[inline(never)]
fn widen(x: &Widen) -> u64 {
    x.w as u64
}

#[inline(never)]
fn outer(x: &Outer) -> u64 {
    x.inner.v[0] + x.inner.v.len() as u64 * 10
}

#[inline(never)]
fn gen_read<T: Copy + Into<u64>>(g: &Gen<T>) -> u64 {
    g.x[0].into() + g.x.len() as u64 * 10
}

#[inline(never)]
fn const_read() -> u64 {
    CONST_READ_SIZE as u64
}

#[inline(never)]
fn const_fn() -> u64 {
    CONST_FN_SIZE as u64
}

#[inline(never)]
fn in_static() -> u64 {
    IN_STATIC.s as u64
}

#[inline(never)]
fn uses_import(x: &UsesImport) -> u64 {
    std::mem::size_of_val(&x.t) as u64
}

pub fn row(late: bool) -> Vec<(&'static str, u64)> {
    let f = Frame::new();
    let mut set = Frame::new();
    set.set();
    vec![
        ("fe_method", f.method()),
        ("fe_free", free(&f)),
        ("fe_pattern", pattern(&f)),
        ("fe_closure", closure(&f)),
        ("fe_generic", generic::<u32>(&f)),
        ("fe_inlined", inlined(&f)),
        ("fe_macro", by_macro(&f)),
        ("fe_update", Frame::updated().method()),
        ("fe_update_self", f.updated_self().method()),
        ("fe_clone", f.clone().method()),
        ("fe_eq", (set != f) as u64),
        ("fe_debug", format!("{f:?}").matches('7').count() as u64),
        ("fe_nested", nested(&Holds { frame: Frame::new() })),
        ("fe_dyn", by_dyn(&f)),
        ("fe_write", set.method()),
        ("fe_held", held().method()),
        ("fe_rest", f.rest()),
        ("fe_late", if late { f.late() } else { 0 }),
        ("field_same_size", same_size(&same_size_new())),
        ("field_two_kinds", two_kinds(&two_kinds_new())),
        ("field_order", order(&order_new())),
        ("field_widen", widen(&widen_new()) * 10 + widen(widen_held())),
        ("field_nested", outer(&outer_new())),
        ("field_generic", gen_read(&Gen::new())),
        ("field_offset", by_offset(&offset_read_new())),
        ("field_const_read", const_read()),
        ("field_const_fn", const_fn()),
        ("field_static", in_static()),
        ("field_import", uses_import(&uses_import_new())),
        ("field_enum", pay_read(&pay_new())),
    ]
}
