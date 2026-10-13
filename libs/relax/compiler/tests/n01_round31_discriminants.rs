#![allow(dead_code)]
const fn tag() -> i16 { -17 }
#[repr(i16)]
enum Signed { A = tag(), B, C = 126, D }
#[repr(u64)]
enum Unsigned { A = 0xffff_ffff_ffff_fffe, B }
#[repr(i16)]
enum Data { A(u8) = -2, B(u16), C = 300 }
#[test]
fn queried_discriminants() {
    assert_eq!(Signed::A as i16, -17);
    assert_eq!(Signed::B as i64, -16);
    assert_eq!(Signed::D as i16, 127);
    assert_eq!(Unsigned::B as u64, u64::MAX);
    assert_eq!(match Data::A(7) { Data::A(n) => n, _ => 0 }, 7);
    assert_eq!(match Data::B(300) { Data::B(n) => n, _ => 0 }, 300);
}
