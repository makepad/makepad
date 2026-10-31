// Layout queries and generated draw-layout blocks must evaluate to the same data
// as their JIT initializers. Run with MAKEPAD_RUN_CTFE=check and rustc --test -O.
use std::marker::PhantomData;
use std::mem::{align_of, offset_of, size_of};

#[repr(C)]
struct Header { tag: u8, word: u32 }
#[repr(C)]
struct Draw<T> { header: Header, value: T, tail: [u16; 3] }
#[repr(C, packed)]
struct Packed { tag: u8, word: u32 }
#[repr(C)]
union Overlay { byte: u8, word: u64 }

trait Layout {
    const SIZE: usize;
    const ALIGN: usize;
}
impl<T> Layout for Draw<T> {
    const SIZE: usize = size_of::<Self>();
    const ALIGN: usize = align_of::<Self>();
}
impl<T> Draw<T> {
    const OFFSET: usize = offset_of!(Self, value);
    const NESTED: usize = offset_of!(Self, header.word);
}

const fn rounded<T>(mut size: usize) -> usize {
    let a = align_of::<T>();
    size = size.div_ceil(a) * a + size_of::<T>();
    size
}
const PADDING: usize = {
    let mut size = 0usize;
    let mut alignment = 4usize;
    {
        let a = align_of::<Header>();
        alignment = if a > alignment { a } else { alignment };
        size = size.div_ceil(a) * a + size_of::<Header>();
    }
    size = rounded::<u64>(size);
    size += size_of::<[u16; 3]>();
    (alignment - size % alignment) % alignment / 2
};
const QUERIES: [usize; 10] = [
    <Draw<u64> as Layout>::SIZE, <Draw<u64> as Layout>::ALIGN,
    Draw::<u64>::OFFSET, Draw::<u64>::NESTED,
    offset_of!(Draw<u64>, tail), offset_of!(Packed, word),
    offset_of!(Overlay, word), size_of::<PhantomData<u64>>(),
    align_of::<[u64; 0]>(), offset_of!((u64, u64), 1),
];
const CEILS: [usize; 4] = [0usize.div_ceil(4), 1usize.div_ceil(4), 8usize.div_ceil(4), usize::MAX.div_ceil(2)];
const BRANCH: u8 = { let mut n = 10u8; if false { n += 255; } else { n -= 3; } n };
const MINIMA: (i8, i64, i128) = (-128, -9223372036854775808, -170141183460469231731687303715884105728);
const ORDER: i32 = { let mut n = 1; n += { n = n + 4; 2 }; n };

#[test]
fn layouts_and_blocks() {
    println!("{:?} {} {:?} {} {}", QUERIES, PADDING, CEILS, BRANCH, ORDER);
    assert_eq!(QUERIES, [24, 8, 8, 4, 16, 1, 0, 0, 8, 8]);
    assert_eq!(PADDING, 1);
    assert_eq!(CEILS, [0, 1, 2, usize::MAX / 2 + 1]);
    assert_eq!(BRANCH, 7);
    assert_eq!(ORDER, 7);
    assert_eq!(MINIMA, (i8::MIN, i64::MIN, i128::MIN));
}
