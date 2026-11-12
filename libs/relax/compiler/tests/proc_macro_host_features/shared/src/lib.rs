// with `rate`: the trait (type namespace) and its derive (macro namespace) under one name
#[cfg(feature = "rate")]
pub use ::hf_trait::Rate;
#[cfg(feature = "rate")]
use hf_derive::Rate;

#[cfg_attr(feature = "rate", derive(Rate))]
pub struct Meter;

pub fn base() -> u32 {
    5
}
