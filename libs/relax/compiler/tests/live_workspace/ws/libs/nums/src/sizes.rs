use super::{UInt, UTerm, B0, B1};
pub type U1 = UInt<UTerm, B1>;
pub type U2 = UInt<U1, B0>;
pub type U3 = UInt<U1, B1>;
pub type U4 = UInt<U2, B0>;
