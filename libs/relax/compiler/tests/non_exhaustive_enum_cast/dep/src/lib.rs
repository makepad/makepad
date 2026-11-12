// Adding variants does not invalidate casts of the existing payload-free variants.
#[non_exhaustive]
#[repr(u32)]
pub enum State {
    First = 3,
    Second = 7,
}

#[non_exhaustive]
#[repr(i32)]
pub enum Signed {
    Negative = -1,
    Positive = 2,
}

// Adding fields to a variant does invalidate a cast outside this crate.
pub enum VariantOnly {
    #[non_exhaustive]
    Unit,
}

pub fn variant_only() -> VariantOnly {
    VariantOnly::Unit
}

pub fn local_variant_cast() -> u32 {
    VariantOnly::Unit as u32
}
