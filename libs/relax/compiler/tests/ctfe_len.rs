// F34: array lengths / repeat counts that need const fn evaluation (`SLICE.len()`, a const fn
// call) are evaluated by running the const (rustc CTFE), also when other consts wait on them.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum K { A, B, C }
pub const VARIANTS: &[K] = &[K::A, K::B, K::C];
const COUNT: usize = VARIANTS.len();
const fn name(k: K) -> u32 { match k { K::A => 10, K::B => 20, K::C => 30 } }
const IDS: [u32; COUNT] = {
    let mut ids = [0u32; COUNT];
    let mut i = 0;
    while i < COUNT { ids[i] = name(VARIANTS[i]); i += 1; }
    ids
};
const fn double(n: usize) -> usize { n * 2 }
const TWICE: usize = double(COUNT);
fn f() -> [u8; TWICE] { [7u8; TWICE] }
#[test]
fn ctfe_len() {
    assert_eq!(IDS, [10, 20, 30]);
    let a = [1u16; COUNT];
    assert_eq!(a.len(), 3);
    assert_eq!(f().len(), 6);
    assert_eq!(f()[5], 7);
}
#[derive(Clone, Copy)]
enum Stage { A, B, End }
const STAGES: usize = Stage::End as usize + 1;
struct Frame { times: [u64; STAGES], tag: u8 }
#[test]
fn ctfe_field_len() {
    let f = Frame { times: std::array::from_fn(|i| i as u64 * 2), tag: 9 };
    assert_eq!(f.times, [0, 2, 4]);
    assert_eq!(f.tag, 9);
    assert_eq!(std::mem::size_of::<Frame>(), 32);
    let _ = (Stage::A, Stage::B);
}

// A const generic argument naming a const whose initializer calls a const fn (`peek_bits::
// <LITLEN_DECODE_BITS>()` in zune-inflate, uizoo): the turbofish arg parses as a type path,
// the const is evaluated by running it and the body checked again.
const fn min_usize(a: usize, b: usize) -> usize { if a < b { a } else { b } }
const PEEK_A: usize = 11;
pub const PEEK_BITS: usize = min_usize(15, 12);
struct Bits { buffer: u64 }
impl Bits { fn peek<const L: usize>(&self) -> usize { (self.buffer & ((1 << L) - 1)) as usize } }
#[test] fn const_fn_const_generic_arg() { let s = Bits { buffer: 0xffff }; assert_eq!(s.peek::<PEEK_BITS>(), 0xfff); assert_eq!(s.peek::<PEEK_A>(), 0x7ff); }
