// N:rustc_ty_utils/src/layout.rs:63-75: consumers inspect the normalized
// layout type, including native associated-constant array lengths.
struct Info;
impl Info {
    const LEN: usize = 3;
    const DATA: [u16; Self::LEN] = [17, 29, 43];
}
#[test]
fn associated_const_array_has_defined_bytes() {
    let data = Info::DATA;
    assert_eq!(data[0], 17);
    assert_eq!(data[1], 29);
    assert_eq!(data[2], 43);
}
