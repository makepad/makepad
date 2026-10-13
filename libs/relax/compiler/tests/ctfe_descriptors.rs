// Static identity, descriptor arrays, enum niches, const methods and fn entries.
#[derive(Clone, Copy)]
struct Field { name: &'static str, offset: usize, callback: Option<fn(u32) -> u32> }
#[derive(Clone, Copy)]
struct Descriptor { name: &'static str, fields: &'static [Field], callback: fn(u32) -> u32 }
impl Descriptor {
    const fn named(mut self, name: &'static str) -> Self { self.name = name; self }
}
fn increment(n: u32) -> u32 { n + 1 }
trait Callback { fn apply(n: u32) -> u32; }
impl Callback for u32 { fn apply(n: u32) -> u32 { n * 2 } }
static FIELDS: [Field; 2] = [
    Field { name: "first", offset: 0, callback: Some(increment) },
    Field { name: "second", offset: 8, callback: None },
];
struct Link { next: &'static Link }
static LINK: Link = Link { next: &LINK };
const ROOT: &Link = &LINK;
const SECOND: &Field = &FIELDS[1];
const SECOND_NAME: &'static str = FIELDS[1].name;
const OFFSET_REF: &usize = &FIELDS[1].offset;
const UNSAFE_FN: unsafe fn(u32) -> u32 = increment;
static BASE: Descriptor = Descriptor { name: "base", fields: &FIELDS, callback: <u32 as Callback>::apply };
const DESCRIPTOR: &Descriptor = {
    static INFO: Descriptor = Descriptor { name: "updated", ..BASE };
    &INFO
};
const NAMED: Descriptor = Descriptor { name: "before", fields: &FIELDS, callback: increment }.named("after");
const CLOSURE: fn(u32) -> u32 = |n| n + 3;
const NESTED: &[&[Field]] = &[&FIELDS, &[Field { name: "promoted", offset: 4, callback: Some(increment) }]];
const LOCAL: (u32, [u32; 2]) = {
    let mut pair = (1u32, [2u32, 3]);
    pair.0 += 4;
    pair.1[1] = 9;
    pair
};
const BORROWED: u32 = { let mut n = 2u32; let r = &n; let before = *r; n = 8; before + n };
const NULL: *const u8 = std::ptr::null();
const SENTINEL: *const u8 = -1isize as *const u8;
const TEXT_BYTES: &[u8] = "fields".as_bytes();
const LENGTHS: [usize; 3] = ["fields".len(), TEXT_BYTES.len(), [1u16, 2, 3].len()];
struct Message { name: *const std::ffi::c_char }
unsafe impl Sync for Message {}
static MESSAGE: Message = Message { name: c"message".as_ptr() };
const C_STRING: &std::ffi::CStr = cr"raw message";
const BITS: [u32; 3] = [f32::INFINITY.to_bits(), f32::NEG_INFINITY.to_bits(), f32::from_bits(0x3f800000).to_bits()];

#[test]
fn descriptors() {
    println!("{} {} {} {} {:?} {} {:?}", DESCRIPTOR.name, NAMED.name, (DESCRIPTOR.callback)(6), CLOSURE(4), LOCAL, BORROWED, BITS);
    assert!(std::ptr::eq(ROOT, ROOT.next));
    assert!(std::ptr::eq(SECOND, &FIELDS[1]));
    assert!(std::ptr::eq(OFFSET_REF, &FIELDS[1].offset));
    assert_eq!(SECOND_NAME, "second");
    assert_eq!(unsafe { UNSAFE_FN(1) }, 2);
    assert_eq!(DESCRIPTOR.name, "updated");
    assert!(std::ptr::eq(DESCRIPTOR.fields.as_ptr(), FIELDS.as_ptr()));
    assert_eq!((DESCRIPTOR.fields[0].callback.unwrap())(4), 5);
    assert!(DESCRIPTOR.fields[1].callback.is_none());
    assert_eq!(NESTED[1][0].name, "promoted");
    assert_eq!(NESTED[1][0].offset, 4);
    assert_eq!(NAMED.name, "after");
    assert_eq!((DESCRIPTOR.callback)(6), 12);
    assert_eq!(CLOSURE(4), 7);
    assert_eq!(LOCAL, (5, [2, 9]));
    assert_eq!(BORROWED, 10);
    assert!(NULL.is_null());
    assert_eq!(SENTINEL as usize, usize::MAX);
    assert_eq!(LENGTHS, [6, 6, 3]);
    assert_eq!(TEXT_BYTES, b"fields");
    assert_eq!(unsafe { std::ffi::CStr::from_ptr(MESSAGE.name) }.to_bytes(), b"message");
    assert_eq!(C_STRING.to_bytes(), b"raw message");
    assert_eq!(BITS, [0x7f800000, 0xff800000, 0x3f800000]);
}
