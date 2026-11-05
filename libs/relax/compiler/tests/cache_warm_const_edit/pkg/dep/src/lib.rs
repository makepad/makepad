pub const DEP_VALUE: u64 = 1;
pub struct DepT;
pub trait DepKind {
    const DK: u64;
}
#[inline(never)]
pub fn read_dk<T: DepKind>() -> u64 {
    T::DK
}

// One const, every way code reads it from another crate (the probe const_every).
pub const EVERY: u64 = 1;
pub const EVERY_DERIVED: u64 = EVERY + 1;
#[inline(never)]
pub fn every_fn() -> u64 {
    EVERY
}
#[inline]
pub fn every_small() -> u64 {
    EVERY
}
#[inline(never)]
pub fn every_generic<T: Default>() -> u64 {
    let _ = T::default();
    EVERY
}
pub trait EveryK {
    const E: u64 = EVERY;
}
pub struct EveryT;
impl EveryK for EveryT {}
#[inline(never)]
pub fn every_late() -> u64 {
    EVERY
}

// Types of a crate the warm run does not check.
pub const DEP_LEN: usize = 1;
#[repr(C)]
pub struct DepBuf {
    pub data: [u8; DEP_LEN],
}
pub const DEP_GEN: usize = 1;
pub struct DepG<const N: usize>;
impl<const N: usize> DepG<N> {
    pub fn get() -> u64 {
        N as u64
    }
}
#[inline(never)]
pub fn dep_generic() -> u64 {
    DepG::<DEP_GEN>::get()
}
pub const DEP_FOR_MAIN: usize = 1;
#[repr(C)]
pub struct DepArr<const N: usize> {
    pub d: [u8; N],
}
pub const DEP_FIELD: usize = 1;
#[repr(C)]
pub struct DepHolder {
    pub x: DepArr<DEP_FIELD>,
}
pub const DEP_ALIAS: usize = 1;
pub type DepAl = DepArr<DEP_ALIAS>;
