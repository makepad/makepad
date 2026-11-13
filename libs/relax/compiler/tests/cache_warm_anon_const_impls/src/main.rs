// Impls whose self types differ only in an array length written as a constant (a free const, an
// associated const of another type: crypto-common's `[u8; U4::USIZE]`), trait and inherent ones.
// Each length is an anonymous constant of its impl; they had one cache identity, so a warm run
// gave every length one value: the wrong impl's code (`[u8; FOUR]` printing 2), an ambiguous
// instance, and in a larger graph E0119 / E0592 between the impls (relax issue 142).
pub trait Unsigned {
    const USIZE: usize;
}
pub struct U2;
impl Unsigned for U2 {
    const USIZE: usize = 2;
}
pub struct U4;
impl Unsigned for U4 {
    const USIZE: usize = 4;
}
pub const TWO: usize = 2;
pub const FOUR: usize = 4;

pub trait Width {
    fn width() -> usize;
}
impl Width for [u8; FOUR] {
    fn width() -> usize {
        4
    }
}
impl Width for [u8; TWO] {
    fn width() -> usize {
        2
    }
}
impl Width for [u16; U4::USIZE] {
    fn width() -> usize {
        8
    }
}
impl Width for [u16; U2::USIZE] {
    fn width() -> usize {
        4
    }
}

pub struct Bytes<T>(pub T);
impl Bytes<[u8; FOUR]> {
    pub fn len(&self) -> usize {
        40
    }
}
impl Bytes<[u8; TWO]> {
    pub fn len(&self) -> usize {
        20
    }
}

fn width<T: Width>() -> usize {
    T::width()
}

fn main() {
    let widths = [width::<[u8; 4]>(), width::<[u8; 2]>(), width::<[u16; 4]>(), width::<[u16; 2]>()];
    let lens = [Bytes([0u8; 4]).len(), Bytes([0u8; 2]).len()];
    println!("cache_warm_anon_const_impls {widths:?} {lens:?}");
}
