#![allow(dead_code)]
#[repr(u128)] enum Unsigned { A = 340282366920938463463374607431768211454u128, B }
#[repr(i128)] enum Signed { A = -170141183460469231731687303715884105728i128, B }
#[repr(u128)] enum Data { A(u8) = 0x10000000000000001, B(u16) = 0x20000000000000001 }
struct Count<'a>(&'a std::cell::Cell<u32>, u32);
impl Drop for Count<'_> { fn drop(&mut self) { self.0.set(self.0.get() + self.1); } }
#[repr(u128)] enum Dropped<'a> { A(Count<'a>) = 0x10000000000000001, B(Count<'a>) = 0x20000000000000001 }
#[repr(i8)] enum Single { A = -2 }
#[test] fn wide_discriminants() {
    assert_eq!(Unsigned::A as u128, u128::MAX - 1);
    assert_eq!(Unsigned::B as u128, u128::MAX);
    assert_eq!(Signed::A as i128, i128::MIN);
    assert_eq!(Signed::B as i128, i128::MIN + 1);
    assert_eq!(std::mem::size_of::<Unsigned>(), 16);
    assert_eq!(match Data::B(400) { Data::A(_) => 0, Data::B(n) => n }, 400);
    assert_eq!(Single::A as i128, -2);
    let count = std::cell::Cell::new(0);
    drop(Dropped::A(Count(&count, 3)));
    drop(Dropped::B(Count(&count, 7)));
    assert_eq!(count.get(), 10);
}
