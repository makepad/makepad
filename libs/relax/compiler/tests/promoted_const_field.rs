// `&[C.id, 6]`: an array whose operand is a constant's field is promoted (rustc promote_consts
// validates a field projection of the constant's temporary), so a function may return it as
// `&'static`. Kept on the stack, the slice read the next calls' frames once the function returned.
pub struct Attr {
    pub name: &'static str,
    pub id: u64,
}
pub const COLOR: Attr = Attr { name: "Vertex_Color", id: 5 };
pub const PAIR: (u64, u32) = (7, 8);

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Quant {
    Unorm8,
    Unorm16,
}

impl Attr {
    pub const WEIGHT: Attr = Attr { name: "Vertex_JointWeight", id: 9 };
}

#[inline(never)]
fn const_field() -> &'static [u64] {
    &[COLOR.id, 6]
}

#[inline(never)]
fn tuple_fields() -> &'static [u64] {
    &[PAIR.0, PAIR.1 as u64]
}

#[inline(never)]
fn field_pairs() -> &'static [(u64, Quant)] {
    &[(COLOR.id, Quant::Unorm8), (Attr::WEIGHT.id, Quant::Unorm16)]
}

#[inline(never)]
fn names() -> &'static [&'static str] {
    &[COLOR.name, Attr::WEIGHT.name]
}

// overwrites the stack below the caller's frame, where a returned temporary would have lived
#[inline(never)]
fn clobber() -> u64 {
    let mut a = [0xa5a5_a5a5_a5a5_a5a5u64; 256];
    for (i, x) in a.iter_mut().enumerate() {
        *x ^= i as u64;
    }
    std::hint::black_box(&mut a).iter().fold(0, |s, x| s ^ x)
}

#[test]
fn promoted_arrays_of_constant_fields_outlive_their_fn() {
    let (a, b, c, d) = (const_field(), tuple_fields(), field_pairs(), names());
    std::hint::black_box(clobber());
    let _ = vec![0u64; 64];
    assert_eq!(a, &[5, 6], "a struct constant's field");
    assert_eq!(b, &[7, 8], "a tuple constant's fields");
    assert_eq!(c, &[(5, Quant::Unorm8), (9, Quant::Unorm16)], "fields of associated constants in tuples");
    assert_eq!(d, &["Vertex_Color", "Vertex_JointWeight"], "`&str` fields");
}
