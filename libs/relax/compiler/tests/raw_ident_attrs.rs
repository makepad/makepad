// raw identifiers in attributes are the plain names (rustc; tests/ui rfc-2151-raw-identifiers/attr.rs):
// `#[r#repr(r#C, r#packed)]` packs, `#[r#derive(r#Debug)]` derives
#[r#repr(r#C, r#packed)]
struct Packed { _a: bool, _b: u64 }
#[r#derive(r#Debug, r#Clone)]
struct T2(#[allow(dead_code)] u32);
#[test]
fn raw_attrs() {
    assert_eq!(std::mem::size_of::<Packed>(), 9);
    assert_eq!(format!("{:?}", T2(123).clone()), "T2(123)");
}
